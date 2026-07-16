#import <Foundation/Foundation.h>
#import <Security/Security.h>
#import <xpc/xpc.h>
#include <dispatch/dispatch.h>
#include <errno.h>
#include <libproc.h>
#include <mach/message.h>
#include <signal.h>
#include <spawn.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef SETWRIGHT_APP_GROUP
#define SETWRIGHT_APP_GROUP "group.org.setwright.desktop"
#endif
#ifndef SETWRIGHT_HOST_REQUIREMENT
#define SETWRIGHT_HOST_REQUIREMENT "identifier \"org.setwright.desktop\""
#endif
#ifndef SETWRIGHT_HELPER_REQUIREMENT
#define SETWRIGHT_HELPER_REQUIREMENT "entitlement[\"com.apple.security.inherit\"] exists"
#endif

extern char **environ;

@interface SWJob : NSObject
@property(nonatomic) pid_t pid;
@property(nonatomic) uint64_t memoryLimit;
@property(nonatomic) uint64_t writableLimit;
@property(nonatomic) uint32_t processLimit;
@property(nonatomic, copy) NSString *outputRoot;
@property(nonatomic) BOOL completed;
@property(nonatomic) int exitCode;
@end
@implementation SWJob
@end

static dispatch_queue_t jobsQueue;
static NSMutableDictionary<NSString *, SWJob *> *jobs;

static void reply_error(xpc_object_t request, const char *message) {
  xpc_object_t reply = xpc_dictionary_create_reply(request);
  if (reply == NULL) return;
  xpc_dictionary_set_string(reply, "error", message ?: "XPC service error");
  xpc_connection_t peer = xpc_dictionary_get_remote_connection(request);
  xpc_connection_send_message(peer, reply);
  xpc_release(reply);
}

static BOOL path_is_within(NSString *path, NSString *root) {
  NSString *resolvedPath = path.stringByResolvingSymlinksInPath.stringByStandardizingPath;
  NSString *resolvedRoot = root.stringByResolvingSymlinksInPath.stringByStandardizingPath;
  if ([resolvedPath isEqualToString:resolvedRoot]) return YES;
  return [resolvedPath hasPrefix:[resolvedRoot stringByAppendingString:@"/"]];
}

static BOOL verify_code(NSString *path, NSString *requirementText) {
  SecStaticCodeRef code = NULL;
  OSStatus status = SecStaticCodeCreateWithPath(
      (__bridge CFURLRef)[NSURL fileURLWithPath:path], kSecCSDefaultFlags, &code);
  if (status != errSecSuccess || code == NULL) return NO;
  SecRequirementRef requirement = NULL;
  status = SecRequirementCreateWithString((__bridge CFStringRef)requirementText,
                                          kSecCSDefaultFlags, &requirement);
  if (status == errSecSuccess)
    status = SecStaticCodeCheckValidity(code, kSecCSStrictValidate, requirement);
  if (requirement != NULL) CFRelease(requirement);
  CFRelease(code);
  return status == errSecSuccess;
}

static NSArray<NSNumber *> *process_tree(pid_t root) {
  NSMutableArray<NSNumber *> *result = [NSMutableArray array];
  NSMutableArray<NSNumber *> *pending = [NSMutableArray arrayWithObject:@(root)];
  while (pending.count > 0) {
    NSNumber *next = pending.lastObject;
    [pending removeLastObject];
    if ([result containsObject:next]) continue;
    [result addObject:next];
    int count = proc_listchildpids(next.intValue, NULL, 0);
    if (count <= 0) continue;
    pid_t *children = calloc((size_t)count, sizeof(pid_t));
    int bytes = proc_listchildpids(next.intValue, children,
                                   count * (int)sizeof(pid_t));
    if (bytes > 0) {
      int childCount = bytes / (int)sizeof(pid_t);
      for (int index = 0; index < childCount; index++)
        [pending addObject:@(children[index])];
    }
    free(children);
  }
  return result;
}

static uint64_t resident_bytes(NSArray<NSNumber *> *processes, BOOL *valid) {
  uint64_t total = 0;
  *valid = YES;
  for (NSNumber *process in processes) {
    struct rusage_info_v2 usage = {};
    if (proc_pid_rusage(process.intValue, RUSAGE_INFO_V2,
                        (rusage_info_t *)&usage) != 0) {
      *valid = NO;
      return 0;
    }
    if (__builtin_add_overflow(total, usage.ri_resident_size, &total)) {
      *valid = NO;
      return 0;
    }
  }
  return total;
}

static uint64_t directory_bytes(NSString *root, BOOL *valid) {
  uint64_t total = 0;
  *valid = YES;
  NSDirectoryEnumerator *enumerator = [[NSFileManager defaultManager]
      enumeratorAtURL:[NSURL fileURLWithPath:root]
      includingPropertiesForKeys:@[NSURLIsRegularFileKey, NSURLFileSizeKey]
      options:NSDirectoryEnumerationSkipsPackageDescendants |
              NSDirectoryEnumerationSkipsHiddenFiles
      errorHandler:^BOOL(NSURL *url, NSError *error) {
        (void)url; (void)error; *valid = NO; return NO;
      }];
  for (NSURL *url in enumerator) {
    NSNumber *regular = nil;
    NSNumber *size = nil;
    if (![url getResourceValue:&regular forKey:NSURLIsRegularFileKey error:nil] ||
        ![url getResourceValue:&size forKey:NSURLFileSizeKey error:nil]) {
      *valid = NO;
      return 0;
    }
    if (regular.boolValue && __builtin_add_overflow(total, size.unsignedLongLongValue, &total)) {
      *valid = NO;
      return 0;
    }
  }
  return total;
}

static void start_watchdog(NSString *jobId, SWJob *job) {
  dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    while (!job.completed) {
      NSArray<NSNumber *> *processes = process_tree(job.pid);
      BOOL memoryValid = NO;
      BOOL outputValid = NO;
      uint64_t memory = resident_bytes(processes, &memoryValid);
      uint64_t output = directory_bytes(job.outputRoot, &outputValid);
      if (!memoryValid || !outputValid || processes.count > job.processLimit ||
          memory > job.memoryLimit || output > job.writableLimit) {
        kill(-job.pid, SIGKILL);
        break;
      }
      usleep(10000);
    }
    (void)jobId;
  });
}

static char **copy_string_array(xpc_object_t array, size_t prefixCount,
                                const char **prefix) {
  size_t count = xpc_array_get_count(array);
  char **result = calloc(prefixCount + count + 1, sizeof(char *));
  if (result == NULL) return NULL;
  for (size_t index = 0; index < prefixCount; index++) result[index] = strdup(prefix[index]);
  for (size_t index = 0; index < count; index++) {
    const char *value = xpc_array_get_string(array, index);
    if (value == NULL) goto fail;
    result[prefixCount + index] = strdup(value);
  }
  return result;
fail:
  for (size_t index = 0; index < prefixCount + count; index++) free(result[index]);
  free(result);
  return NULL;
}

static char **copy_environment(xpc_object_t dictionary, NSString *runtime,
                               NSString *output) {
  __block size_t count = 0;
  xpc_dictionary_apply(dictionary, ^bool(const char *key, xpc_object_t value) {
    if (xpc_get_type(value) != XPC_TYPE_STRING || strchr(key, '=') != NULL) return false;
    count++;
    return true;
  });
  char **environment = calloc(count + 4, sizeof(char *));
  if (environment == NULL) return NULL;
  __block size_t index = 0;
  __block BOOL valid = YES;
  xpc_dictionary_apply(dictionary, ^bool(const char *key, xpc_object_t value) {
    const char *text = xpc_string_get_string_ptr(value);
    if (text == NULL || strchr(key, '=') != NULL) { valid = NO; return false; }
    if (asprintf(&environment[index++], "%s=%s", key, text) < 0) {
      valid = NO; return false;
    }
    return true;
  });
  if (asprintf(&environment[index++], "PATH=%s/bin", runtime.UTF8String) < 0 ||
      asprintf(&environment[index++], "TMPDIR=%s", output.UTF8String) < 0)
    valid = NO;
  if (valid) return environment;
  for (size_t item = 0; item < index; item++) free(environment[item]);
  free(environment);
  return NULL;
}

static void free_strings(char **values) {
  if (values == NULL) return;
  for (size_t index = 0; values[index] != NULL; index++) free(values[index]);
  free(values);
}

static void handle_launch(xpc_object_t request) {
  const char *runtimeText = xpc_dictionary_get_string(request, "runtimeRoot");
  const char *stageText = xpc_dictionary_get_string(request, "stageRoot");
  const char *outputText = xpc_dictionary_get_string(request, "outputRoot");
  xpc_object_t arguments = xpc_dictionary_get_value(request, "arguments");
  xpc_object_t environment = xpc_dictionary_get_value(request, "environment");
  if (runtimeText == NULL || stageText == NULL || outputText == NULL ||
      xpc_get_type(arguments) != XPC_TYPE_ARRAY ||
      xpc_get_type(environment) != XPC_TYPE_DICTIONARY) {
    reply_error(request, "invalid XPC launch request");
    return;
  }
  NSString *runtime = [NSString stringWithUTF8String:runtimeText];
  NSString *stage = [NSString stringWithUTF8String:stageText];
  NSString *output = [NSString stringWithUTF8String:outputText];
  NSURL *groupURL = [[NSFileManager defaultManager]
      containerURLForSecurityApplicationGroupIdentifier:@SETWRIGHT_APP_GROUP];
  NSString *appRoot = NSBundle.mainBundle.bundlePath.stringByDeletingLastPathComponent
      .stringByDeletingLastPathComponent.stringByDeletingLastPathComponent;
  NSString *allowedRuntime = [appRoot stringByAppendingPathComponent:@"Contents/Resources/tex-runtime"];
  NSString *latexmk = [runtime stringByAppendingPathComponent:@"bin/latexmk"];
  NSString *launcher = [NSBundle.mainBundle pathForResource:@"setwright-tex-launcher"
                                                      ofType:nil];
  if (groupURL == nil) {
    reply_error(request, "XPC app-group container is unavailable");
    return;
  }
  if (!path_is_within(stage, groupURL.path)) {
    reply_error(request, "XPC stage is outside the app-group container");
    return;
  }
  if (!path_is_within(output, stage)) {
    reply_error(request, "XPC output is outside the compile stage");
    return;
  }
  if (!path_is_within(runtime, allowedRuntime)) {
    reply_error(request, "XPC runtime is outside the signed host bundle");
    return;
  }
  if (launcher == nil || !verify_code(launcher, @SETWRIGHT_HELPER_REQUIREMENT)) {
    reply_error(request, "XPC signed inheriting launcher requirement failed");
    return;
  }
  if (!verify_code(latexmk, @SETWRIGHT_HELPER_REQUIREMENT)) {
    reply_error(request, "XPC signed TeX helper requirement failed");
    return;
  }

  uint64_t memory = xpc_dictionary_get_uint64(request, "memoryLimit");
  uint64_t writable = xpc_dictionary_get_uint64(request, "writableLimit");
  uint64_t processLimit = xpc_dictionary_get_uint64(request, "processLimit");
  char memoryText[32], writableText[32];
  snprintf(memoryText, sizeof(memoryText), "%llu", memory);
  snprintf(writableText, sizeof(writableText), "%llu", writable);
  const char *prefix[] = {launcher.UTF8String, "--setwright-fixed-launch-v1",
                          memoryText, writableText, latexmk.UTF8String};
  char **argv = copy_string_array(arguments, 5, prefix);
  char **envp = copy_environment(environment, runtime, output);
  int stdoutPipe[2] = {-1, -1};
  int stderrPipe[2] = {-1, -1};
  if (argv == NULL || envp == NULL || pipe(stdoutPipe) != 0 || pipe(stderrPipe) != 0) {
    free_strings(argv); free_strings(envp);
    if (stdoutPipe[0] >= 0) close(stdoutPipe[0]);
    if (stdoutPipe[1] >= 0) close(stdoutPipe[1]);
    if (stderrPipe[0] >= 0) close(stderrPipe[0]);
    if (stderrPipe[1] >= 0) close(stderrPipe[1]);
    reply_error(request, "cannot allocate XPC launch state");
    return;
  }
  posix_spawn_file_actions_t actions;
  posix_spawnattr_t attributes;
  posix_spawn_file_actions_init(&actions);
  posix_spawn_file_actions_adddup2(&actions, stdoutPipe[1], STDOUT_FILENO);
  posix_spawn_file_actions_adddup2(&actions, stderrPipe[1], STDERR_FILENO);
  posix_spawn_file_actions_addclose(&actions, stdoutPipe[0]);
  posix_spawn_file_actions_addclose(&actions, stderrPipe[0]);
  posix_spawn_file_actions_addchdir_np(&actions, stage.UTF8String);
  posix_spawnattr_init(&attributes);
  posix_spawnattr_setflags(&attributes, POSIX_SPAWN_START_SUSPENDED |
                                        POSIX_SPAWN_SETPGROUP |
                                        POSIX_SPAWN_CLOEXEC_DEFAULT);
  posix_spawnattr_setpgroup(&attributes, 0);
  pid_t pid = 0;
  int spawned = posix_spawn(&pid, launcher.UTF8String, &actions, &attributes, argv, envp);
  posix_spawnattr_destroy(&attributes);
  posix_spawn_file_actions_destroy(&actions);
  close(stdoutPipe[1]); close(stderrPipe[1]);
  free_strings(argv); free_strings(envp);
  if (spawned != 0) {
    close(stdoutPipe[0]); close(stderrPipe[0]);
    reply_error(request, "cannot start signed inheriting TeX helper");
    return;
  }
  NSString *jobId = NSUUID.UUID.UUIDString;
  SWJob *job = [SWJob new];
  job.pid = pid;
  job.memoryLimit = memory;
  job.writableLimit = writable;
  job.processLimit = (uint32_t)MIN(processLimit, UINT32_MAX);
  job.outputRoot = output;
  dispatch_sync(jobsQueue, ^{ jobs[jobId] = job; });
  start_watchdog(jobId, job);
  xpc_object_t reply = xpc_dictionary_create_reply(request);
  xpc_dictionary_set_string(reply, "jobId", jobId.UTF8String);
  xpc_dictionary_set_fd(reply, "stdout", stdoutPipe[0]);
  xpc_dictionary_set_fd(reply, "stderr", stderrPipe[0]);
  xpc_connection_send_message(xpc_dictionary_get_remote_connection(request), reply);
  xpc_release(reply);
  close(stdoutPipe[0]); close(stderrPipe[0]);
}

static SWJob *lookup_job(xpc_object_t request, NSString **jobId) {
  const char *text = xpc_dictionary_get_string(request, "jobId");
  if (text == NULL) return nil;
  *jobId = [NSString stringWithUTF8String:text];
  __block SWJob *job = nil;
  dispatch_sync(jobsQueue, ^{ job = jobs[*jobId]; });
  return job;
}

static void handle_control(xpc_object_t request, NSString *operation) {
  NSString *jobId = nil;
  SWJob *job = lookup_job(request, &jobId);
  if (job == nil) { reply_error(request, "unknown XPC compile job"); return; }
  if ([operation isEqualToString:@"resume"]) {
    if (kill(job.pid, SIGCONT) != 0) { reply_error(request, "cannot resume XPC job"); return; }
  } else if ([operation isEqualToString:@"terminate"] && !job.completed) {
    if (kill(-job.pid, SIGKILL) != 0 && errno != ESRCH) {
      reply_error(request, "cannot terminate XPC process group"); return;
    }
  }
  xpc_object_t reply = xpc_dictionary_create_reply(request);
  xpc_connection_send_message(xpc_dictionary_get_remote_connection(request), reply);
  xpc_release(reply);
}

static void handle_wait(xpc_object_t request) {
  NSString *jobId = nil;
  SWJob *job = lookup_job(request, &jobId);
  if (job == nil) { reply_error(request, "unknown XPC compile job"); return; }
  xpc_retain(request);
  dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    int status = 0;
    if (waitpid(job.pid, &status, 0) < 0) {
      reply_error(request, "cannot wait for XPC compiler process");
    } else {
      job.completed = YES;
      job.exitCode = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
      xpc_object_t reply = xpc_dictionary_create_reply(request);
      xpc_dictionary_set_int64(reply, "exitCode", job.exitCode);
      xpc_connection_send_message(xpc_dictionary_get_remote_connection(request), reply);
      xpc_release(reply);
      dispatch_sync(jobsQueue, ^{ [jobs removeObjectForKey:jobId]; });
    }
    xpc_release(request);
  });
}

static void handle_message(xpc_object_t request) {
  const char *operationText = xpc_dictionary_get_string(request, "operation");
  if (operationText == NULL) { reply_error(request, "missing XPC operation"); return; }
  NSString *operation = [NSString stringWithUTF8String:operationText];
  if ([operation isEqualToString:@"launch"]) handle_launch(request);
  else if ([operation isEqualToString:@"wait"]) handle_wait(request);
  else if ([operation isEqualToString:@"resume"] ||
           [operation isEqualToString:@"terminate"]) handle_control(request, operation);
  else reply_error(request, "unsupported XPC operation");
}

static void accept_peer(xpc_connection_t peer) {
  if (xpc_connection_set_peer_code_signing_requirement(
          peer, SETWRIGHT_HOST_REQUIREMENT) != 0) {
    xpc_connection_cancel(peer);
    return;
  }
  xpc_connection_set_event_handler(peer, ^(xpc_object_t event) {
    if (xpc_get_type(event) == XPC_TYPE_DICTIONARY) handle_message(event);
  });
  xpc_connection_activate(peer);
}

int main(void) {
  @autoreleasepool {
    jobsQueue = dispatch_queue_create("org.setwright.compiler-xpc.jobs", DISPATCH_QUEUE_SERIAL);
    jobs = [NSMutableDictionary dictionary];
    xpc_main(accept_peer);
  }
  return 0;
}
