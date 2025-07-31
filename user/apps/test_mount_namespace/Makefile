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

TARGET = test_mount_namespace
SRC = src/main.c
DESTDIR = $(DADK_CURRENT_BUILD_DIR)

.PHONY: all clean install

all: $(TARGET)

$(TARGET): $(SRC)
	$(CC) $(CFLAGS) -o $(TARGET) $(SRC) $(LDFLAGS)

clean:
	rm -f $(TARGET)

install: $(TARGET)
	echo "Install to $(DESTDIR)"
	mkdir -p $(DESTDIR)
	cp $(TARGET) $(DESTDIR)/

# For debugging
debug: CFLAGS += -g -DDEBUG
debug: $(TARGET)

.DEFAULT_GOAL := all