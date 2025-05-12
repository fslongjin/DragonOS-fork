ifeq ($(ARCH), x86_64)
	CROSS_COMPILE=x86_64-linux-musl-
else ifeq ($(ARCH), riscv64)
	CROSS_COMPILE=riscv64-linux-musl-
endif

CC=$(CROSS_COMPILE)gcc
TEST_DIR_LOCAL=tests/test_pagecache
TEST_DIR=$(DADK_CURRENT_BUILD_DIR)/$(TEST_DIR_LOCAL)
FILENAME=test_pagecache

.PHONY: all
all: main.c generate_test_file
	$(CC) -static -o $(FILENAME) main.c libmd5-c/libmd5-c.a

.PHONY: install clean
install: all generate_test_file
	mkdir -p $(DADK_CURRENT_BUILD_DIR)/bin
	mv $(FILENAME) $(DADK_CURRENT_BUILD_DIR)/bin/$(FILENAME)


	mkdir -p $(TEST_DIR)
	cp -r $(TEST_DIR_LOCAL)/* $(TEST_DIR)
	@echo "Test files copied to build directory."

generate_test_file:
	@echo "Generating test file..."
	mkdir -p $(TEST_DIR_LOCAL)
# 创建一个10KB的测试文件，内容是随机数
	dd if=/dev/urandom of=$(TEST_DIR_LOCAL)/test_file bs=1K count=10
	@echo "Test file generated."
# 计算md5sum
	md5sum $(TEST_DIR_LOCAL)/test_file > $(TEST_DIR_LOCAL)/test_file.md5sum


clean:
	rm $(FILENAME) *.o
	$(MAKE) -C libmd5-c clean

fmt:
