#include <errno.h>
#include <fcntl.h>
#include <seccomp.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

static void deny_syscall(scmp_filter_ctx context, const char *name) {
  int syscall_number = seccomp_syscall_resolve_name(name);
  if (syscall_number == __NR_SCMP_ERROR) return;
  int result = seccomp_rule_add(context, SCMP_ACT_ERRNO(EPERM), syscall_number, 0);
  if (result != 0) {
    fprintf(stderr, "cannot deny %s: %d\n", name, result);
    exit(1);
  }
}

int main(int argc, char **argv) {
  if (argc != 2) {
    fprintf(stderr, "usage: %s OUTPUT.bpf\n", argv[0]);
    return 64;
  }
  scmp_filter_ctx context = seccomp_init(SCMP_ACT_ALLOW);
  if (context == NULL) {
    fputs("cannot allocate seccomp filter\n", stderr);
    return 1;
  }
  const char *denied[] = {
      "add_key",          "bpf",          "delete_module",
      "finit_module",     "init_module",  "io_pgetevents",
      "io_uring_enter",   "io_uring_register", "io_uring_setup",
      "kexec_file_load",  "kexec_load",   "keyctl",
      "mount",            "move_mount",   "name_to_handle_at",
      "open_by_handle_at", "open_tree",   "perf_event_open",
      "pivot_root",       "process_vm_readv", "process_vm_writev",
      "ptrace",           "request_key",  "setns",
      "socket",           "socketcall",   "swapoff",
      "swapon",           "umount",       "umount2",
      "unshare",          "userfaultfd",
  };
  for (size_t index = 0; index < sizeof(denied) / sizeof(denied[0]); index++)
    deny_syscall(context, denied[index]);
  if (seccomp_attr_set(context, SCMP_FLTATR_CTL_NNP, 1) != 0) {
    fputs("cannot require no_new_privs in seccomp filter\n", stderr);
    seccomp_release(context);
    return 1;
  }
  int output = open(argv[1], O_CREAT | O_EXCL | O_WRONLY | O_CLOEXEC, 0444);
  if (output < 0) {
    perror("open seccomp output");
    seccomp_release(context);
    return 1;
  }
  int result = seccomp_export_bpf(context, output);
  if (close(output) != 0 && result == 0) result = -errno;
  seccomp_release(context);
  if (result != 0) {
    fprintf(stderr, "cannot export seccomp filter: %d\n", result);
    unlink(argv[1]);
    return 1;
  }
  return 0;
}
