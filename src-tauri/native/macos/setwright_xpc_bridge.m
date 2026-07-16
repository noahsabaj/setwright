#import <Foundation/Foundation.h>
#import <Security/Security.h>
#import <xpc/xpc.h>
#include <errno.h>
#include <libproc.h>
#include <limits.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

typedef struct {
  xpc_connection_t connection;
} sw_xpc_client;

static void sw_error(char *buffer, size_t length, NSString *message) {
  if (buffer == NULL || length == 0) return;
  snprintf(buffer, length, "%s", message.UTF8String ?: "unknown XPC error");
}

static NSString *sw_cf_error(CFErrorRef error, NSString *fallback) {
  if (error == NULL) return fallback;
  return CFBridgingRelease(CFErrorCopyDescription(error));
}

int sw_xpc_verify_service(const char *bundle_path, const char *requirement_text,
                          const char *app_group, char *error_buffer,
                          size_t error_length) {
  @autoreleasepool {
    if (bundle_path == NULL || requirement_text == NULL || app_group == NULL) {
      sw_error(error_buffer, error_length, @"XPC verification received a null value");
      return EINVAL;
    }
    NSString *path = [NSString stringWithUTF8String:bundle_path];
    if (![path.pathExtension isEqualToString:@"xpc"] ||
        ![[NSFileManager defaultManager] fileExistsAtPath:path]) {
      sw_error(error_buffer, error_length, @"embedded XPC service bundle is missing");
      return ENOENT;
    }
    SecStaticCodeRef code = NULL;
    OSStatus status = SecStaticCodeCreateWithPath(
        (__bridge CFURLRef)[NSURL fileURLWithPath:path], kSecCSDefaultFlags, &code);
    if (status != errSecSuccess || code == NULL) {
      sw_error(error_buffer, error_length, @"cannot open XPC service code signature");
      return (int)status;
    }
    SecRequirementRef requirement = NULL;
    status = SecRequirementCreateWithString(
        (__bridge CFStringRef)[NSString stringWithUTF8String:requirement_text],
        kSecCSDefaultFlags, &requirement);
    if (status != errSecSuccess || requirement == NULL) {
      CFRelease(code);
      sw_error(error_buffer, error_length, @"XPC service code requirement is invalid");
      return (int)status;
    }
    CFErrorRef validity_error = NULL;
    status = SecStaticCodeCheckValidityWithErrors(
        code, kSecCSCheckAllArchitectures | kSecCSStrictValidate, requirement,
        &validity_error);
    CFRelease(requirement);
    if (status != errSecSuccess) {
      sw_error(error_buffer, error_length,
               sw_cf_error(validity_error, @"XPC service signature is invalid"));
      if (validity_error != NULL) CFRelease(validity_error);
      CFRelease(code);
      return (int)status;
    }
    if (validity_error != NULL) CFRelease(validity_error);

    CFDictionaryRef signing = NULL;
    status = SecCodeCopySigningInformation(code, kSecCSSigningInformation, &signing);
    CFRelease(code);
    if (status != errSecSuccess || signing == NULL) {
      sw_error(error_buffer, error_length, @"cannot inspect XPC service entitlements");
      return (int)status;
    }
    NSDictionary *information = (__bridge NSDictionary *)signing;
    NSDictionary *entitlements = information[(__bridge NSString *)kSecCodeInfoEntitlementsDict];
    NSString *group = [NSString stringWithUTF8String:app_group];
    BOOL sandboxed = [entitlements[@"com.apple.security.app-sandbox"] boolValue];
    BOOL networked = [entitlements[@"com.apple.security.network.client"] boolValue] ||
                     [entitlements[@"com.apple.security.network.server"] boolValue];
    NSArray *groups = entitlements[@"com.apple.security.application-groups"];
    BOOL has_group = [groups isKindOfClass:[NSArray class]] && [groups containsObject:group];
    CFRelease(signing);
    if (!sandboxed || networked || !has_group) {
      sw_error(error_buffer, error_length,
               @"XPC service must be sandboxed, networkless, and stage-app-group scoped");
      return EPERM;
    }
    return 0;
  }
}

void *sw_xpc_connect(const char *service_name, char *error_buffer,
                     size_t error_length) {
  if (service_name == NULL) {
    sw_error(error_buffer, error_length, @"XPC service name is null");
    return NULL;
  }
  xpc_connection_t connection = xpc_connection_create(service_name, NULL);
  if (connection == NULL) {
    sw_error(error_buffer, error_length, @"cannot create XPC service connection");
    return NULL;
  }
  xpc_connection_set_event_handler(connection, ^(xpc_object_t event) {
    (void)event;
  });
  xpc_connection_activate(connection);
  sw_xpc_client *client = calloc(1, sizeof(sw_xpc_client));
  if (client == NULL) {
    xpc_release(connection);
    sw_error(error_buffer, error_length, @"cannot allocate XPC client state");
    return NULL;
  }
  client->connection = connection;
  return client;
}

void sw_xpc_disconnect(void *opaque) {
  sw_xpc_client *client = opaque;
  if (client == NULL) return;
  xpc_connection_cancel(client->connection);
  xpc_release(client->connection);
  free(client);
}

static xpc_object_t sw_send(sw_xpc_client *client, xpc_object_t message,
                            char *error_buffer, size_t error_length) {
  if (client == NULL) {
    sw_error(error_buffer, error_length, @"XPC client is disconnected");
    return NULL;
  }
  xpc_object_t reply = xpc_connection_send_message_with_reply_sync(
      client->connection, message);
  if (reply == NULL || xpc_get_type(reply) == XPC_TYPE_ERROR) {
    const char *description = reply == NULL ? NULL :
        xpc_dictionary_get_string(reply, XPC_ERROR_KEY_DESCRIPTION);
    sw_error(error_buffer, error_length,
             description == NULL ? @"XPC service did not reply" :
             [NSString stringWithUTF8String:description]);
    if (reply != NULL) xpc_release(reply);
    return NULL;
  }
  const char *remote_error = xpc_dictionary_get_string(reply, "error");
  if (remote_error != NULL) {
    sw_error(error_buffer, error_length,
             [NSString stringWithUTF8String:remote_error]);
    xpc_release(reply);
    return NULL;
  }
  return reply;
}

int sw_xpc_launch(void *opaque, const char *runtime_root, const char *stage_root,
                  const char *output_root, const char *const *arguments,
                  size_t argument_count, const char *const *environment_keys,
                  const char *const *environment_values, size_t environment_count,
                  uint64_t memory_limit, uint64_t writable_limit,
                  uint32_t process_limit, int *stdout_fd, int *stderr_fd,
                  int32_t *root_pid, char *job_id, size_t job_id_length,
                  char *error_buffer, size_t error_length) {
  sw_xpc_client *client = opaque;
  xpc_object_t message = xpc_dictionary_create(NULL, NULL, 0);
  xpc_dictionary_set_string(message, "operation", "launch");
  xpc_dictionary_set_string(message, "runtimeRoot", runtime_root);
  xpc_dictionary_set_string(message, "stageRoot", stage_root);
  xpc_dictionary_set_string(message, "outputRoot", output_root);
  xpc_dictionary_set_uint64(message, "memoryLimit", memory_limit);
  xpc_dictionary_set_uint64(message, "writableLimit", writable_limit);
  xpc_dictionary_set_uint64(message, "processLimit", process_limit);
  xpc_object_t args = xpc_array_create(NULL, 0);
  for (size_t index = 0; index < argument_count; index++)
    xpc_array_set_string(args, XPC_ARRAY_APPEND, arguments[index]);
  xpc_dictionary_set_value(message, "arguments", args);
  xpc_release(args);
  xpc_object_t environment = xpc_dictionary_create(NULL, NULL, 0);
  for (size_t index = 0; index < environment_count; index++)
    xpc_dictionary_set_string(environment, environment_keys[index],
                              environment_values[index]);
  xpc_dictionary_set_value(message, "environment", environment);
  xpc_release(environment);
  xpc_object_t reply = sw_send(client, message, error_buffer, error_length);
  xpc_release(message);
  if (reply == NULL) return EIO;
  const char *remote_job_id = xpc_dictionary_get_string(reply, "jobId");
  int64_t remote_pid = xpc_dictionary_get_int64(reply, "pid");
  int out = xpc_dictionary_dup_fd(reply, "stdout");
  int err = xpc_dictionary_dup_fd(reply, "stderr");
  if (remote_job_id == NULL || remote_pid <= 0 || remote_pid > INT32_MAX ||
      out < 0 || err < 0 || root_pid == NULL) {
    if (out >= 0) close(out);
    if (err >= 0) close(err);
    xpc_release(reply);
    sw_error(error_buffer, error_length, @"XPC launch reply is incomplete");
    return EPROTO;
  }
  snprintf(job_id, job_id_length, "%s", remote_job_id);
  *root_pid = (int32_t)remote_pid;
  *stdout_fd = out;
  *stderr_fd = err;
  xpc_release(reply);
  return 0;
}

static int sw_job_command(void *opaque, const char *operation,
                          const char *job_id, const char *reason,
                          int32_t *exit_code,
                          char *error_buffer, size_t error_length) {
  sw_xpc_client *client = opaque;
  xpc_object_t message = xpc_dictionary_create(NULL, NULL, 0);
  xpc_dictionary_set_string(message, "operation", operation);
  xpc_dictionary_set_string(message, "jobId", job_id);
  if (reason != NULL) xpc_dictionary_set_string(message, "reason", reason);
  xpc_object_t reply = sw_send(client, message, error_buffer, error_length);
  xpc_release(message);
  if (reply == NULL) return EIO;
  if (exit_code != NULL)
    *exit_code = (int32_t)xpc_dictionary_get_int64(reply, "exitCode");
  xpc_release(reply);
  return 0;
}

int sw_xpc_resume(void *opaque, const char *job_id, char *error_buffer,
                  size_t error_length) {
  return sw_job_command(opaque, "resume", job_id, NULL, NULL,
                        error_buffer, error_length);
}

int sw_xpc_terminate(void *opaque, const char *job_id, const char *reason,
                     char *error_buffer, size_t error_length) {
  return sw_job_command(opaque, "terminate", job_id, reason, NULL,
                        error_buffer, error_length);
}

int sw_xpc_wait(void *opaque, const char *job_id, int32_t *exit_code,
                char *error_buffer, size_t error_length) {
  return sw_job_command(opaque, "wait", job_id, NULL, exit_code,
                        error_buffer, error_length);
}

int sw_process_tree_resident_bytes(int32_t root_pid, uint32_t process_limit,
                                   uint64_t *resident_bytes,
                                   char *error_buffer, size_t error_length) {
  @autoreleasepool {
    if (root_pid <= 0 || process_limit == 0 || process_limit > 4096 ||
        resident_bytes == NULL) {
      sw_error(error_buffer, error_length, @"invalid process-tree memory request");
      return EINVAL;
    }
    NSMutableSet<NSNumber *> *seen = [NSMutableSet set];
    NSMutableArray<NSNumber *> *pending =
        [NSMutableArray arrayWithObject:@(root_pid)];
    uint64_t total = 0;
    while (pending.count > 0) {
      NSNumber *next = pending.lastObject;
      [pending removeLastObject];
      if ([seen containsObject:next]) continue;
      if (seen.count >= process_limit) {
        sw_error(error_buffer, error_length,
                 @"process tree exceeded its bounded memory-inspection limit");
        return EOVERFLOW;
      }
      [seen addObject:next];

      struct rusage_info_v2 usage = {};
      errno = 0;
      if (proc_pid_rusage(next.intValue, RUSAGE_INFO_V2,
                          (rusage_info_t *)&usage) != 0) {
        if (errno == ESRCH && next.intValue != root_pid) continue;
        sw_error(error_buffer, error_length,
                 @"cannot inspect process-tree resident memory");
        return errno == 0 ? EIO : errno;
      }
      if (__builtin_add_overflow(total, usage.ri_resident_size, &total)) {
        sw_error(error_buffer, error_length,
                 @"process-tree resident memory overflowed");
        return EOVERFLOW;
      }

      errno = 0;
      int reported = proc_listchildpids(next.intValue, NULL, 0);
      if (reported < 0) {
        if (errno == ESRCH && next.intValue != root_pid) continue;
        sw_error(error_buffer, error_length,
                 @"cannot enumerate process-tree descendants");
        return errno == 0 ? EIO : errno;
      }
      if (reported == 0) continue;
      NSUInteger outstanding = seen.count + pending.count;
      NSUInteger remaining = outstanding < process_limit
          ? process_limit - outstanding : 0;
      size_t slots = MIN((size_t)reported, (size_t)remaining + 1);
      pid_t *children = calloc(slots, sizeof(pid_t));
      if (children == NULL) {
        sw_error(error_buffer, error_length,
                 @"cannot allocate bounded process-tree sample");
        return ENOMEM;
      }
      int child_count = proc_listchildpids(
          next.intValue, children, (int)(slots * sizeof(pid_t)));
      if (child_count < 0) {
        int child_error = errno;
        free(children);
        if (child_error == ESRCH && next.intValue != root_pid) continue;
        sw_error(error_buffer, error_length,
                 @"cannot enumerate process-tree descendants");
        return child_error == 0 ? EIO : child_error;
      }
      for (int index = 0; index < child_count; index++) {
        NSNumber *child = @(children[index]);
        if (children[index] <= 0 || [seen containsObject:child] ||
            [pending containsObject:child]) continue;
        if (seen.count + pending.count >= process_limit) {
          free(children);
          sw_error(error_buffer, error_length,
                   @"process tree exceeded its bounded memory-inspection limit");
          return EOVERFLOW;
        }
        [pending addObject:child];
      }
      free(children);
    }
    *resident_bytes = total;
    return 0;
  }
}
