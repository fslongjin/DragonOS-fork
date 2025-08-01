ifeq ($(ARCH), )
	ARCH = x86_64
endif

ifeq ($(ARCH), x86_64)
	CROSS_COMPILE = x86_64-linux-musl-
else ifeq ($(ARCH), riscv64)
	CROSS_COMPILE = riscv64-linux-musl-
endif

CC = $(CROSS_COMPILE)gcc

CFLAGS = -std=gnu99 -mcmodel=large -fno-builtin -fno-stack-protector -static -Wall -Wextra
LDFLAGS = -static

TARGET1 = test_mount_namespace
TARGET2 = test_propagation
TARGET3 = comprehensive_propagation_test
TARGET4 = simple_test
SRC1 = src/main.c
SRC2 = src/test_propagation.c
SRC3 = src/comprehensive_propagation_test.c
SRC4 = src/simple_test.c
DESTDIR = $(DADK_CURRENT_BUILD_DIR)

.PHONY: all clean install

all: $(TARGET1) $(TARGET2) $(TARGET3) $(TARGET4)

$(TARGET1): $(SRC1)
	$(CC) $(CFLAGS) -o $(TARGET1) $(SRC1) $(LDFLAGS)

$(TARGET2): $(SRC2)
	$(CC) $(CFLAGS) -o $(TARGET2) $(SRC2) $(LDFLAGS)

$(TARGET3): $(SRC3)
	$(CC) $(CFLAGS) -o $(TARGET3) $(SRC3) $(LDFLAGS)

$(TARGET4): $(SRC4)
	$(CC) $(CFLAGS) -o $(TARGET4) $(SRC4) $(LDFLAGS)

clean:
	rm -f $(TARGET1) $(TARGET2) $(TARGET3) $(TARGET4)

install: $(TARGET1) $(TARGET2) $(TARGET3) $(TARGET4)
	echo "Install to $(DESTDIR)"
	mkdir -p $(DESTDIR)
	cp $(TARGET1) $(DESTDIR)/
	cp $(TARGET2) $(DESTDIR)/
	cp $(TARGET3) $(DESTDIR)/
	cp $(TARGET4) $(DESTDIR)/

# For debugging
debug: CFLAGS += -g -DDEBUG
debug: $(TARGET1) $(TARGET2) $(TARGET3) $(TARGET4)

# Individual targets for convenience
test_basic: $(TARGET1)

test_simple: $(TARGET2)

test_comprehensive: $(TARGET3)

test_debug: $(TARGET4)

# Build only specific tests
comprehensive: $(TARGET3)

simple: $(TARGET4)

# Help target
help:
	@echo "Available targets:"
	@echo "  all                     - Build all test programs"
	@echo "  test_basic              - Build basic mount namespace test"
	@echo "  test_simple             - Build simple propagation test"
	@echo "  test_comprehensive     - Build comprehensive propagation test"
	@echo "  test_debug              - Build debug/problem-focused test"
	@echo "  comprehensive           - Alias for test_comprehensive"
	@echo "  simple                  - Alias for test_debug"
	@echo "  debug                   - Build all with debug symbols"
	@echo "  clean                   - Remove all built files"
	@echo "  install                 - Install all programs to DESTDIR"
	@echo "  help                    - Show this help message"
	@echo ""
	@echo "Test programs:"
	@echo "  $(TARGET1)     - Basic mount namespace isolation test"
	@echo "  $(TARGET2)          - Simple propagation functionality test"
	@echo "  $(TARGET3) - Comprehensive propagation test suite"
	@echo "  $(TARGET4)              - Debug test for specific failing cases"

.DEFAULT_GOAL := all