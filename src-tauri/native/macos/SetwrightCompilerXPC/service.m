#import <Foundation/Foundation.h>
#import <Security/Security.h>
#import <xpc/xpc.h>
#include <dispatch/dispatch.h>
#include <errno.h>
#include <libproc.h>
#include <mach/message.h>
#include <os/log.h>
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
#include <time.h>
#include <unistd.h>

#ifndef SETWRIGHT_APP_GROUP
#define SETWRIGHT_APP_GROUP "group.org.setwright.desktop"
#endif
#ifndef SETWRIGHT_HOST_REQUIREMENT
// Packaging must replace the placeholder OU with the release Team ID. Leaving
// this default in place is deliberately fail-closed.
#define SETWRIGHT_HOST_REQUIREMENT \
  "anchor apple generic and certificate leaf[subject.OU] = \"SETWRIGHT_TEAM_ID_REQUIRED\" and identifier \"org.setwright.desktop\""
#endif
#ifndef SETWRIGHT_HELPER_REQUIREMENT
#define SETWRIGHT_HELPER_REQUIREMENT \
  "anchor apple generic and certificate leaf[subject.OU] = \"SETWRIGHT_TEAM_ID_REQUIRED\" and entitlement[\"com.apple.security.inherit\"] exists"
#endif

static const uint64_t SWBrokerWaitRegistrationNanos = 30ULL * 1000000000ULL;

extern char **environ;

@interface SWJob : NSObject
@property(nonatomic) pid_t pid;
@property(nonatomic) uint64_t writableLimit;
@property(nonatomic) uint32_t processLimit;
@property(nonatomic) uint64_t wallDeadlineNanos;
@property(nonatomic, copy) NSString *outputRoot;
@property(atomic) BOOL completed;
@property(atomic) BOOL resumed;
@property(atomic) BOOL waiting;
@property(atomic) BOOL watchdogReady;
@property(atomic) BOOL lifecycleTransactionOpen;
@property(atomic, copy) NSString *terminationDetail;
@property(nonatomic) int diagnosticFd;
@property(nonatomic) int exitCode;
@end
@implementation SWJob
@end

static dispatch_queue_t jobsQueue;
static NSMutableDictionary<NSString *, SWJob *> *jobs;

static BOOL monotonic_nanoseconds(uint64_t *value) {
  if (value == NULL) return NO;
  struct timespec now = {};
  if (clock_gettime(CLOCK_MONOTONIC, &now) != 0 || now.tv_sec < 0 ||
      now.tv_nsec < 0) return NO;
  uint64_t seconds = 0;
  return !__builtin_mul_overflow((uint64_t)now.tv_sec, 1000000000ULL,
                                 &seconds) &&
         !__builtin_add_overflow(seconds, (uint64_t)now.tv_nsec, value);
}

static void reply_error(xpc_object_t request, const char *message) {
  xpc_object_t reply = xpc_dictionary_create_reply(request);
  if (reply == NULL) return;
  xpc_dictionary_set_string(reply, "error", message ?: "XPC service error");
  xpc_connection_t peer = xpc_dictionary_get_remote_connection(request);
  xpc_connection_send_message(peer, reply);
  xpc_release(reply);
}

static void reply_ok(xpc_object_t request) {
  xpc_object_t reply = xpc_dictionary_create_reply(request);
  if (reply == NULL) return;
  xpc_connection_send_message(xpc_dictionary_get_remote_connection(request), reply);
  xpc_release(reply);
}

static BOOL path_is_within(NSString *path, NSString *root) {
  NSString *resolvedPath = path.stringByResolvingSymlinksInPath.stringByStandardizingPath;
  NSString *resolvedRoot = root.stringByResolvingSymlinksInPath.stringByStandardizingPath;
  if ([resolvedPath isEqualToString:resolvedRoot]) return YES;
  return [resolvedPath hasPrefix:[resolvedRoot stringByAppendingString:@"/"]];
}

static OSStatus verify_code(NSString *path, NSString *requirementText) {
  SecStaticCodeRef code = NULL;
  OSStatus status = SecStaticCodeCreateWithPath(
      (__bridge CFURLRef)[NSURL fileURLWithPath:path], kSecCSDefaultFlags, &code);
  if (status != errSecSuccess || code == NULL)
    return status == errSecSuccess ? errSecParam : status;
  SecRequirementRef requirement = NULL;
  status = SecRequirementCreateWithString((__bridge CFStringRef)requirementText,
                                          kSecCSDefaultFlags, &requirement);
  if (status == errSecSuccess)
    status = SecStaticCodeCheckValidity(code, kSecCSStrictValidate, requirement);
  if (requirement != NULL) CFRelease(requirement);
  CFRelease(code);
  return status;
}

typedef struct {
  BOOL valid;
  BOOL exceeded;
  NSUInteger count;
} SWProcessInspection;

// App Sandbox permits the service to enumerate its own descendants but does
// not grant task-inspection access for proc_pid_rusage. Keep the process-count
// enforcement here and bound every allocation/traversal by the configured
// limit so a fork storm cannot livelock the watchdog itself.
static SWProcessInspection inspect_process_limit(pid_t root, uint32_t limit) {
  SWProcessInspection inspection = {.valid = YES, .exceeded = NO, .count = 0};
  if (root <= 0 || limit == 0) {
    inspection.valid = NO;
    return inspection;
  }
  NSMutableSet<NSNumber *> *seen = [NSMutableSet set];
  NSMutableArray<NSNumber *> *pending = [NSMutableArray arrayWithObject:@(root)];
  while (pending.count > 0) {
    NSNumber *next = pending.lastObject;
    [pending removeLastObject];
    if ([seen containsObject:next]) continue;
    if (seen.count >= limit) {
      inspection.exceeded = YES;
      inspection.count = seen.count + 1;
      return inspection;
    }
    [seen addObject:next];

    errno = 0;
    int reported = proc_listchildpids(next.intValue, NULL, 0);
    if (reported < 0) {
      if (errno == ESRCH && next.intValue != root) continue;
      inspection.valid = NO;
      return inspection;
    }
    if (reported == 0) continue;

    NSUInteger outstanding = seen.count + pending.count;
    NSUInteger remaining = outstanding < limit ? limit - outstanding : 0;
    // One extra slot is sufficient to prove a violation and keeps the buffer
    // bounded even if the hostile tree is growing while it is sampled.
    size_t slots = MIN((size_t)reported, (size_t)remaining + 1);
    pid_t *children = calloc(slots, sizeof(pid_t));
    if (children == NULL) {
      inspection.valid = NO;
      return inspection;
    }
    // Unlike proc_listpids, proc_listchildpids returns a PID count.
    int childCount = proc_listchildpids(next.intValue, children,
                                        (int)(slots * sizeof(pid_t)));
    if (childCount < 0) {
      free(children);
      if (errno == ESRCH && next.intValue != root) continue;
      inspection.valid = NO;
      return inspection;
    }
    for (int index = 0; index < childCount; index++) {
      NSNumber *child = @(children[index]);
      if (children[index] <= 0 || [seen containsObject:child] ||
          [pending containsObject:child]) continue;
      if (seen.count + pending.count >= limit) {
        free(children);
        inspection.exceeded = YES;
        inspection.count = seen.count + pending.count + 1;
        return inspection;
      }
      [pending addObject:child];
    }
    free(children);
  }
  inspection.count = seen.count;
  return inspection;
}

static uint64_t directory_bytes(NSString *root, BOOL *valid) {
  uint64_t total = 0;
  *valid = YES;
  NSDirectoryEnumerator *enumerator = [[NSFileManager defaultManager]
      enumeratorAtURL:[NSURL fileURLWithPath:root]
      includingPropertiesForKeys:@[NSURLIsRegularFileKey, NSURLFileSizeKey]
      options:0
      errorHandler:^BOOL(NSURL *url, NSError *error) {
        (void)url; (void)error; *valid = NO; return NO;
      }];
  if (enumerator == nil) {
    *valid = NO;
    return 0;
  }
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

static int terminate_job(SWJob *job, NSString *detail) {
  @synchronized(job) {
    if (job.completed) return ESRCH;
    if (kill(-job.pid, SIGKILL) != 0) return errno;
    job.terminationDetail = detail;
    return 0;
  }
}

static void end_lifecycle_transaction(SWJob *job) {
  BOOL shouldEnd = NO;
  @synchronized(job) {
    if (job.lifecycleTransactionOpen) {
      job.lifecycleTransactionOpen = NO;
      shouldEnd = YES;
    }
  }
  if (shouldEnd) xpc_transaction_end();
}

static void start_watchdog(NSString *jobId, SWJob *job) {
  dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    job.watchdogReady = YES;
    unsigned invalidSamples = 0;
    while (!job.completed) {
      @autoreleasepool {
        BOOL outputValid = NO;
        SWProcessInspection processes =
            inspect_process_limit(job.pid, job.processLimit);
        uint64_t output = directory_bytes(job.outputRoot, &outputValid);
        uint64_t now = 0;
        BOOL timeValid = monotonic_nanoseconds(&now);
        NSString *violation = nil;
        if (processes.valid && outputValid && timeValid) {
          invalidSamples = 0;
          if (now >= job.wallDeadlineNanos) {
            violation = @"wall-clock compile limit exceeded";
          } else if (processes.exceeded) {
            violation = [NSString stringWithFormat:
                @"process count %lu exceeded limit %u",
                (unsigned long)processes.count, job.processLimit];
          } else if (output > job.writableLimit) {
            violation = [NSString stringWithFormat:
                @"writable output %llu exceeded limit %llu", output,
                job.writableLimit];
          }
        } else if (++invalidSamples >= 3) {
          violation = [NSString stringWithFormat:
              @"resource inspection failed (processes=%d output=%d time=%d)",
              processes.valid, outputValid, timeValid];
        }
        if (violation != nil) {
          int terminationError = terminate_job(
              job, [@"Setwright XPC watchdog: " stringByAppendingString:violation]);
          if (terminationError == 0) {
            os_log_error(OS_LOG_DEFAULT, "Setwright XPC watchdog terminated job: %{public}s",
                         violation.UTF8String);
            break;
          }
          if (terminationError == ESRCH) break;
        }
      }
      usleep(5000);
    }
    (void)jobId;
  });
}

static void reap_job(NSString *jobId, SWJob *job, xpc_object_t reply,
                     xpc_connection_t peer) {
  int status = 0;
  pid_t waited = 0;
  do {
    waited = waitpid(job.pid, &status, 0);
  } while (waited < 0 && errno == EINTR);
  if (waited < 0) {
    terminate_job(job, @"Setwright XPC waiter could not reap the process group");
    job.completed = YES;
    if (reply != NULL)
      xpc_dictionary_set_string(reply, "error",
                                "cannot wait for XPC compiler process");
  } else {
    NSString *terminationDetail = nil;
    @synchronized(job) {
      job.completed = YES;
      terminationDetail = job.terminationDetail;
    }
    job.exitCode = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
    if (WIFSIGNALED(status)) {
      NSString *detail = terminationDetail ?: [NSString stringWithFormat:
          @"Setwright XPC compiler terminated by signal %d", WTERMSIG(status)];
      dprintf(job.diagnosticFd, "%s\n", detail.UTF8String);
    }
    if (reply != NULL) xpc_dictionary_set_int64(reply, "exitCode", job.exitCode);
  }
  close(job.diagnosticFd);
  job.diagnosticFd = -1;
  if (reply != NULL) {
    if (peer != NULL) xpc_connection_send_message(peer, reply);
    xpc_release(reply);
  }
  if (peer != NULL) xpc_release(peer);
  dispatch_sync(jobsQueue, ^{ [jobs removeObjectForKey:jobId]; });
  end_lifecycle_transaction(job);
}

static void start_orphan_reaper(NSString *jobId, SWJob *job,
                                uint64_t delayNanos) {
  dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)delayNanos),
                 dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
    @autoreleasepool {
      BOOL claimed = NO;
      @synchronized(job) {
        if (!job.completed && !job.waiting) {
          job.waiting = YES;
          claimed = YES;
        }
      }
      if (!claimed) return;
      int terminationError = terminate_job(
          job, @"Setwright XPC orphan reaper expired without a broker waiter");
      if (terminationError != 0 && terminationError != ESRCH) {
        os_log_error(OS_LOG_DEFAULT,
                     "Setwright XPC orphan reaper could not terminate job: %{public}d",
                     terminationError);
      }
      reap_job(jobId, job, NULL, NULL);
    }
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
  NSString *allowedRuntime = [NSBundle.mainBundle.bundlePath
      stringByAppendingPathComponent:@"Contents/Resources/tex-runtime"];
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
  OSStatus launcherStatus = launcher == nil
      ? errSecParam : verify_code(launcher, @SETWRIGHT_HELPER_REQUIREMENT);
  if (launcherStatus != errSecSuccess) {
    char message[128];
    snprintf(message, sizeof(message),
             "XPC signed inheriting launcher requirement failed: %d",
             (int)launcherStatus);
    reply_error(request, message);
    return;
  }
  OSStatus helperStatus = verify_code(latexmk, @SETWRIGHT_HELPER_REQUIREMENT);
  if (helperStatus != errSecSuccess) {
    char message[128];
    snprintf(message, sizeof(message),
             "XPC signed TeX helper requirement failed: %d", (int)helperStatus);
    reply_error(request, message);
    return;
  }

  uint64_t timeoutMs = xpc_dictionary_get_uint64(request, "timeoutMs");
  uint64_t memory = xpc_dictionary_get_uint64(request, "memoryLimit");
  uint64_t writable = xpc_dictionary_get_uint64(request, "writableLimit");
  uint64_t processLimit = xpc_dictionary_get_uint64(request, "processLimit");
  uint64_t now = 0;
  uint64_t timeoutNanos = 0;
  uint64_t wallDeadlineNanos = 0;
  if (timeoutMs == 0 || timeoutMs > 24ULL * 60ULL * 60ULL * 1000ULL ||
      memory == 0 || writable == 0 || processLimit == 0 || processLimit > 4096 ||
      !monotonic_nanoseconds(&now) ||
      __builtin_mul_overflow(timeoutMs, 1000000ULL, &timeoutNanos) ||
      __builtin_add_overflow(now, timeoutNanos, &wallDeadlineNanos)) {
    reply_error(request, "invalid XPC resource limits");
    return;
  }
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
  close(stdoutPipe[1]);
  free_strings(argv); free_strings(envp);
  if (spawned != 0) {
    close(stderrPipe[1]);
    close(stdoutPipe[0]); close(stderrPipe[0]);
    reply_error(request, "cannot start signed inheriting TeX helper");
    return;
  }
  NSString *jobId = NSUUID.UUID.UUIDString;
  SWJob *job = [[[SWJob alloc] init] autorelease];
  job.pid = pid;
  job.writableLimit = writable;
  job.processLimit = (uint32_t)MIN(processLimit, UINT32_MAX);
  job.wallDeadlineNanos = wallDeadlineNanos;
  job.outputRoot = output;
  job.diagnosticFd = stderrPipe[1];
  dispatch_sync(jobsQueue, ^{ jobs[jobId] = job; });
  // The suspended process cannot outrun a monitor that is already scheduled.
  // Do not hand launch authority to the broker until the watchdog is live.
  start_watchdog(jobId, job);
  for (unsigned attempt = 0; attempt < 1000 && !job.watchdogReady; attempt++)
    usleep(1000);
  if (!job.watchdogReady) {
    job.completed = YES;
    kill(-pid, SIGKILL);
    waitpid(pid, NULL, 0);
    close(job.diagnosticFd);
    job.diagnosticFd = -1;
    close(stdoutPipe[0]);
    close(stderrPipe[0]);
    dispatch_sync(jobsQueue, ^{ [jobs removeObjectForKey:jobId]; });
    reply_error(request, "XPC resource watchdog did not become ready");
    return;
  }
  xpc_object_t reply = xpc_dictionary_create_reply(request);
  if (reply == NULL) {
    job.completed = YES;
    kill(-pid, SIGKILL);
    waitpid(pid, NULL, 0);
    close(job.diagnosticFd);
    job.diagnosticFd = -1;
    close(stdoutPipe[0]);
    close(stderrPipe[0]);
    dispatch_sync(jobsQueue, ^{ [jobs removeObjectForKey:jobId]; });
    return;
  }
  // The launch reply ends XPC's automatic transaction, but the compiler is
  // still suspended and owned by this service. Keep the service live until a
  // wait request creates the long-lived asynchronous reply transaction.
  xpc_transaction_begin();
  job.lifecycleTransactionOpen = YES;
  start_orphan_reaper(jobId, job, SWBrokerWaitRegistrationNanos);
  xpc_dictionary_set_string(reply, "jobId", jobId.UTF8String);
  xpc_dictionary_set_int64(reply, "pid", pid);
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
  dispatch_sync(jobsQueue, ^{ job = [jobs[*jobId] retain]; });
  return [job autorelease];
}

static void handle_control(xpc_object_t request, NSString *operation) {
  NSString *jobId = nil;
  SWJob *job = lookup_job(request, &jobId);
  if (job == nil) {
    // Tree termination is deliberately idempotent. The waiter may reap and
    // retire a job between a caller observing its marker and sending cancel.
    if ([operation isEqualToString:@"terminate"]) reply_ok(request);
    else reply_error(request, "unknown XPC compile job");
    return;
  }
  if ([operation isEqualToString:@"resume"]) {
    @synchronized(job) {
      if (job.resumed) {
        reply_error(request, "XPC compiler process was already resumed");
        return;
      }
      if (kill(job.pid, SIGCONT) != 0) {
        reply_error(request, "cannot resume XPC job");
        return;
      }
      job.resumed = YES;
    }
  } else if ([operation isEqualToString:@"terminate"]) {
    const char *reasonText = xpc_dictionary_get_string(request, "reason");
    NSString *reason = @"Setwright XPC broker terminated the process group";
    if (reasonText != NULL && strlen(reasonText) > 0 && strlen(reasonText) <= 512) {
      reason = [@"Setwright XPC broker: " stringByAppendingString:
          [NSString stringWithUTF8String:reasonText]];
    }
    int terminationError = terminate_job(
        job, reason);
    if (terminationError != 0 && terminationError != ESRCH) {
      reply_error(request, "cannot terminate XPC process group"); return;
    }
  }
  reply_ok(request);
}

static void handle_wait(xpc_object_t request) {
  NSString *jobId = nil;
  SWJob *job = lookup_job(request, &jobId);
  if (job == nil) { reply_error(request, "unknown XPC compile job"); return; }
  xpc_object_t reply = xpc_dictionary_create_reply(request);
  if (reply == NULL) {
    reply_error(request, "cannot allocate XPC wait reply");
    return;
  }
  xpc_connection_t peer = xpc_dictionary_get_remote_connection(request);
  if (peer == NULL) {
    xpc_release(reply);
    reply_error(request, "XPC wait request has no remote connection");
    return;
  }
  @synchronized(job) {
    if (job.waiting) {
      xpc_release(reply);
      reply_error(request, "XPC compiler process already has a waiter");
      return;
    }
    job.waiting = YES;
  }
  xpc_retain(peer);
  // Creating `reply` keeps the automatic message transaction open while the
  // asynchronous wait runs, so the explicit launch transaction can end now.
  end_lifecycle_transaction(job);
  dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    @autoreleasepool {
      reap_job(jobId, job, reply, peer);
    }
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
    @autoreleasepool {
      if (xpc_get_type(event) == XPC_TYPE_DICTIONARY) handle_message(event);
    }
  });
  xpc_connection_activate(peer);
}

int main(void) {
  @autoreleasepool {
    jobsQueue = dispatch_queue_create("org.setwright.compiler-xpc.jobs", DISPATCH_QUEUE_SERIAL);
    // The XPC service is built without ARC. This registry must outlive every
    // per-message autorelease pool for the lifetime of the service process.
    jobs = [[NSMutableDictionary alloc] init];
    xpc_main(accept_peer);
  }
  return 0;
}
