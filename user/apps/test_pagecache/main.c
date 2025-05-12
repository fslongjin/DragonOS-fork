#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#include "libmd5-c/md5.h"

static int running_in_linux = 0;

static char *TEST_FILE = "/tests/test_pagecache/test_file";
static char *TEST_FILE_MD5SUM = "/tests/test_pagecache/test_file.md5sum";

static char *TEST_FILE_LINUX = "./tests/test_pagecache/test_file";
static char *TEST_FILE_MD5SUM_LINUX = "./tests/test_pagecache/test_file.md5sum";

void run_test(const char *name, int (*test_func)(), int expected) {
  printf("Testing %s... ", name);
  int result = test_func();
  if (result == expected) {
    printf("[PASS]\n");
  } else {
    printf("[FAILED] (expected %d, got %d)\n", expected, result);
  }
}

static void md5_with_buf(char *buf, size_t len, uint8_t *digest) {
  MD5Context ctx;
  md5Init(&ctx);
  md5Update(&ctx, (uint8_t *)buf, len);
  md5Finalize(&ctx);

  memcpy(digest, ctx.digest, 16);
}

int test_md5sum() {
  int fd = open(TEST_FILE, O_RDONLY);
  if (fd < 0) {
    perror("open test file failed");
    return -1;
  }

  struct stat st;
  if (fstat(fd, &st) < 0) {
    perror("fstat failed");
    close(fd);
    return -1;
  }

  void *addr = mmap(NULL, st.st_size, PROT_READ, MAP_SHARED, fd, 0);
  if (addr == MAP_FAILED) {
    perror("mmap failed");
    close(fd);
    return -1;
  }

  uint8_t digest[16];
  md5_with_buf(addr, st.st_size, digest);

  char md5_str[33];
  for (int i = 0; i < 16; i++) {
    sprintf(md5_str + i * 2, "%02x", digest[i]);
  }

  munmap(addr, st.st_size);
  close(fd);

  // Read expected md5sum from file
  FILE *md5sum_file = fopen(TEST_FILE_MD5SUM, "r");
  if (!md5sum_file) {
    perror("open md5sum file failed");
    return -1;
  }

  char expected_md5[33];
  if (fscanf(md5sum_file, "%32s", expected_md5) != 1) {
    perror("read md5sum failed");
    fclose(md5sum_file);
    return -1;
  }
  fclose(md5sum_file);

  return strncmp(md5_str, expected_md5, 32) == 0 ? 0 : -1;
}

static void check_run_os(void) {
  if (getenv("RUN_OS") != NULL && strcmp(getenv("RUN_OS"), "linux") == 0) {
    printf("Running on Linux\n");
    TEST_FILE = TEST_FILE_LINUX;
    TEST_FILE_MD5SUM = TEST_FILE_MD5SUM_LINUX;
  }
}
int main() {

  check_run_os();
  run_test("test_md5sum", test_md5sum, 0);

  return 0;
}
