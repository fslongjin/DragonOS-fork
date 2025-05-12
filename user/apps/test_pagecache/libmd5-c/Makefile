ifeq ($(ARCH), x86_64)
	CROSS_COMPILE=x86_64-linux-musl-
else ifeq ($(ARCH), riscv64)
	CROSS_COMPILE=riscv64-linux-musl-
endif

CC=$(CROSS_COMPILE)gcc
AR=$(CROSS_COMPILE)ar

CFLAGS = -Wall -Wextra -O3

all: md5.c main.c
	@$(CC) $(CFLAGS) -c md5.c -o md5.o
	@$(AR) rcs libmd5-c.a md5.o

clean:
	@$(RM) *.o *.a
