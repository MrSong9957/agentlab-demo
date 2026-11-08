#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <fcntl.h>

int main(void) {
    long raw;
    __asm__ volatile("syscall" : "=a"(raw) : "a"((long)SYS_clone3), "D"(0L), "S"(0L) : "rcx", "r11", "memory");
    errno = 0;
    long result = syscall(SYS_clone3, 0L, 0L, 0L);
    int err = errno;
    printf("clone3 nr=%ld args=(NULL,0) raw_kernel_return=%ld libc_return=%ld errno=%d\n", (long)SYS_clone3, raw, result, err);
    errno = 0;
    result = syscall(437, AT_FDCWD, NULL, NULL, 0L);
    err = errno;
    printf("openat2 nr=437 args=(AT_FDCWD,NULL,NULL,0) libc_return=%ld errno=%d\n", result, err);
    return 0;
}
