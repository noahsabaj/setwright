#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>

static int parse_limit(const char *value, rlim_t *result) {
  if (value == NULL || *value == '\0') return EINVAL;
  errno = 0;
  char *end = NULL;
  unsigned long long parsed = strtoull(value, &end, 10);
  if (errno != 0 || end == value || *end != '\0') return EINVAL;
  *result = (rlim_t)parsed;
  return 0;
}

static int apply_limit(int resource, rlim_t value) {
  struct rlimit limit = {.rlim_cur = value, .rlim_max = value};
  return setrlimit(resource, &limit) == 0 ? 0 : errno;
}

int main(int argc, char **argv, char **environment) {
  if (argc < 5 || strcmp(argv[1], "--setwright-fixed-launch-v1") != 0) {
    fputs("invalid Setwright XPC launcher protocol\n", stderr);
    return 64;
  }
  rlim_t memory = 0;
  rlim_t writable = 0;
  if (parse_limit(argv[2], &memory) != 0 || parse_limit(argv[3], &writable) != 0) {
    fputs("invalid Setwright resource limit\n", stderr);
    return 64;
  }
  // Darwin applies RLIMIT_AS to the launcher's existing VM map and rejects a
  // lower limit with EINVAL. The XPC watchdog enforces aggregate resident
  // memory for the complete process tree instead.
  (void)memory;
  int limit_error = apply_limit(RLIMIT_FSIZE, writable);
  if (limit_error != 0) {
    errno = limit_error;
    perror("setrlimit RLIMIT_FSIZE");
    return 70;
  }
  limit_error = apply_limit(RLIMIT_CORE, 0);
  if (limit_error != 0) {
    errno = limit_error;
    perror("setrlimit RLIMIT_CORE");
    return 70;
  }
  execve(argv[4], &argv[4], environment);
  perror("execve signed runtime helper");
  return 71;
}
