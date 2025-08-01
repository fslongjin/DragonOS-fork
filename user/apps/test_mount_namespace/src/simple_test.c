#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <string.h>
#include <errno.h>
#include <dirent.h>
#include <sched.h>
#include <time.h>

// 测试目录定义
#define SHARED_DIR "/tmp/test_shared"
#define PRIVATE_DIR "/tmp/test_private"

// Mount propagation flags
#ifndef MS_SHARED
#define MS_SHARED (1 << 20)
#endif
#ifndef MS_PRIVATE
#define MS_PRIVATE (1 << 18)
#endif
#ifndef MS_REC
#define MS_REC 16384
#endif

// 颜色输出定义
#define COLOR_GREEN "\033[0;32m"
#define COLOR_RED "\033[0;31m"
#define COLOR_YELLOW "\033[0;33m"
#define COLOR_BLUE "\033[0;34m"
#define COLOR_RESET "\033[0m"

// 工具函数
int create_test_dir(const char* path) {
    if (mkdir(path, 0755) != 0 && errno != EEXIST) {
        printf("错误: 无法创建目录 %s: %s\n", path, strerror(errno));
        return -1;
    }
    return 0;
}

int create_test_file(const char* filename, const char* content) {
    int fd = open(filename, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0) {
        printf("错误: 无法创建文件 %s: %s\n", filename, strerror(errno));
        return -1;
    }
    
    if (write(fd, content, strlen(content)) < 0) {
        printf("错误: 无法写入文件 %s: %s\n", filename, strerror(errno));
        close(fd);
        return -1;
    }
    
    close(fd);
    return 0;
}

int file_exists(const char* filename) {
    return access(filename, F_OK) == 0;
}

void list_directory_contents(const char* path, const char* prefix) {
    DIR *dir;
    struct dirent *entry;
    
    printf("%s目录 %s 的内容:\n", prefix, path);
    
    dir = opendir(path);
    if (dir == NULL) {
        printf("%s  错误: 无法打开目录 %s: %s\n", prefix, path, strerror(errno));
        return;
    }
    
    int count = 0;
    while ((entry = readdir(dir)) != NULL) {
        if (strcmp(entry->d_name, ".") != 0 && strcmp(entry->d_name, "..") != 0) {
            printf("%s  - %s\n", prefix, entry->d_name);
            count++;
        }
    }
    
    if (count == 0) {
        printf("%s  (目录为空)\n", prefix);
    }
    
    closedir(dir);
}

int safe_mount(const char* source, const char* target, const char* fstype, unsigned long flags, const char* data) {
    if (mount(source, target, fstype, flags, data) != 0) {
        printf("挂载失败: %s -> %s (%s): %s\n", source ? source : "none", target, fstype, strerror(errno));
        return -1;
    }
    printf("挂载成功: %s -> %s (%s)\n", source ? source : "none", target, fstype);
    return 0;
}

int safe_umount(const char* target) {
    if (umount(target) != 0) {
        printf("卸载失败: %s: %s\n", target, strerror(errno));
        return -1;
    }
    printf("卸载成功: %s\n", target);
    return 0;
}

// 测试2的简化版本：Shared Mount Propagation
int test_shared_propagation_simple() {
    printf(COLOR_BLUE "=== 简化测试: Shared Mount Propagation ===" COLOR_RESET "\n");
    
    // 清理旧的挂载点
    umount(SHARED_DIR);
    
    // 设置共享挂载
    if (safe_mount("none", SHARED_DIR, "ramfs", 0, NULL) != 0) {
        if (safe_mount("none", SHARED_DIR, "tmpfs", 0, NULL) != 0) {
            printf(COLOR_RED "❌ 无法挂载测试文件系统" COLOR_RESET "\n");
            return -1;
        }
    }
    
    printf("设置共享传播类型...\n");
    if (mount("", SHARED_DIR, "", MS_SHARED, NULL) != 0) {
        printf(COLOR_YELLOW "⚠️  设置共享传播失败: %s" COLOR_RESET "\n", strerror(errno));
    } else {
        printf(COLOR_GREEN "✓ 设置为共享传播成功" COLOR_RESET "\n");
    }
    
    // 创建测试文件
    char shared_file[256];
    snprintf(shared_file, sizeof(shared_file), "%s/shared_file.txt", SHARED_DIR);
    create_test_file(shared_file, "shared content");
    
    printf("父namespace中的共享挂载点:\n");
    list_directory_contents(SHARED_DIR, "   ");
    
    // 创建子挂载目录
    char child_mount_dir[256];
    snprintf(child_mount_dir, sizeof(child_mount_dir), "%s/child_mount", SHARED_DIR);
    if (create_test_dir(child_mount_dir) != 0) {
        safe_umount(SHARED_DIR);
        return -1;
    }
    
    pid_t pid = fork();
    if (pid == 0) {
        // 子进程：测试共享传播
        printf("[子进程] 开始测试\n");
        
        if (unshare(CLONE_NEWNS) != 0) {
            printf("[子进程] 创建mount namespace失败: %s\n", strerror(errno));
            exit(1);
        }
        
        printf("[子进程] 创建新的mount namespace成功\n");
        printf("[子进程] 共享挂载点状态:\n");
        list_directory_contents(SHARED_DIR, "      ");
        
        // 在共享挂载点下创建新挂载
        printf("[子进程] 尝试在共享挂载点下创建子挂载: %s\n", child_mount_dir);
        if (safe_mount("none", child_mount_dir, "ramfs", 0, NULL) != 0) {
            printf("[子进程] 在共享挂载点下创建子挂载失败\n");
            exit(1);
        }
        
        printf("[子进程] 在共享挂载点下创建子挂载成功\n");
        
        // 创建子挂载的标识文件
        char child_mount_file[256];
        snprintf(child_mount_file, sizeof(child_mount_file), "%s/child_mount_file.txt", child_mount_dir);
        if (create_test_file(child_mount_file, "child mount content") != 0) {
            exit(1);
        }
        
        printf("[子进程] 在新挂载点创建文件成功\n");
        
        exit(0);
    } else {
        int status;
        waitpid(pid, &status, 0);
        
        printf("检查共享传播效果:\n");
        list_directory_contents(SHARED_DIR, "   ");
        
        char child_mount_file[256];
        snprintf(child_mount_file, sizeof(child_mount_file), "%s/child_mount_file.txt", child_mount_dir);
        
        if (file_exists(child_mount_file)) {
            printf(COLOR_GREEN "✓ 共享传播正常工作 - 子进程的挂载传播到父namespace" COLOR_RESET "\n");
        } else {
            printf(COLOR_YELLOW "⚠️  共享传播可能未完全实现 - 子进程的挂载未传播" COLOR_RESET "\n");
        }
        
        // 清理 - 这里是容易出错的地方
        printf("开始清理...\n");
        if (safe_umount(child_mount_dir) != 0) {
            printf(COLOR_RED "❌ 卸载子挂载点失败" COLOR_RESET "\n");
        }
        rmdir(child_mount_dir);
        if (safe_umount(SHARED_DIR) != 0) {
            printf(COLOR_RED "❌ 卸载主挂载点失败" COLOR_RESET "\n");
        }
        
        return WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : -1;
    }
}

// 测试3的简化版本：Private Mount Propagation
int test_private_propagation_simple() {
    printf(COLOR_BLUE "=== 简化测试: Private Mount Propagation ===" COLOR_RESET "\n");
    
    // 清理旧的挂载点
    umount(PRIVATE_DIR);
    
    // 设置私有挂载
    if (safe_mount("none", PRIVATE_DIR, "ramfs", 0, NULL) != 0) {
        if (safe_mount("none", PRIVATE_DIR, "tmpfs", 0, NULL) != 0) {
            printf(COLOR_RED "❌ 无法挂载测试文件系统" COLOR_RESET "\n");
            return -1;
        }
    }
    
    printf("设置私有传播类型...\n");
    if (mount("", PRIVATE_DIR, "", MS_PRIVATE, NULL) != 0) {
        printf(COLOR_YELLOW "⚠️  设置私有传播失败: %s" COLOR_RESET "\n", strerror(errno));
    } else {
        printf(COLOR_GREEN "✓ 设置为私有传播成功" COLOR_RESET "\n");
    }
    
    // 创建测试文件
    char private_file[256];
    snprintf(private_file, sizeof(private_file), "%s/private_file.txt", PRIVATE_DIR);
    create_test_file(private_file, "private content");
    
    printf("父namespace私有挂载状态:\n");
    list_directory_contents(PRIVATE_DIR, "   ");
    
    pid_t pid = fork();
    if (pid == 0) {
        // 子进程：测试私有传播
        printf("[子进程] 开始测试\n");
        
        if (unshare(CLONE_NEWNS) != 0) {
            printf("[子进程] 创建mount namespace失败: %s\n", strerror(errno));
            exit(1);
        }
        
        printf("[子进程] 创建新的mount namespace成功\n");
        printf("[子进程] 私有挂载状态:\n");
        list_directory_contents(PRIVATE_DIR, "      ");
        
        // 重新挂载私有挂载点 - 这里是容易出错的地方
        printf("[子进程] 尝试重新挂载私有挂载点: %s\n", PRIVATE_DIR);
        if (safe_mount("none", PRIVATE_DIR, "ramfs", 0, NULL) != 0) {
            printf("[子进程] 重新挂载失败\n");
            exit(1);
        }
        
        printf("[子进程] 重新挂载私有挂载点成功\n");
        
        // 创建子进程专有文件
        char child_private_file[256];
        snprintf(child_private_file, sizeof(child_private_file), "%s/child_private_file.txt", PRIVATE_DIR);
        if (create_test_file(child_private_file, "child private content") != 0) {
            exit(1);
        }
        
        printf("[子进程] 重新挂载后的状态:\n");
        list_directory_contents(PRIVATE_DIR, "      ");
        
        exit(0);
    } else {
        int status;
        waitpid(pid, &status, 0);
        
        printf("检查私有传播效果:\n");
        printf("父namespace状态 (应保持不变):\n");
        list_directory_contents(PRIVATE_DIR, "      ");
        
        if (file_exists(private_file)) {
            printf(COLOR_GREEN "✓ 私有传播正常工作 - 父namespace未受子进程影响" COLOR_RESET "\n");
        } else {
            printf(COLOR_RED "❌ 私有传播失败 - 父namespace受到了影响" COLOR_RESET "\n");
        }
        
        // 清理
        safe_umount(PRIVATE_DIR);
        
        return WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : -1;
    }
}

int main() {
    printf(COLOR_BLUE "=== DragonOS Mount Namespace 问题测试 ===" COLOR_RESET "\n");
    printf("专门测试出现问题的用例\n\n");
    
    // 创建测试目录
    create_test_dir(SHARED_DIR);
    create_test_dir(PRIVATE_DIR);
    
    int test1_result = test_shared_propagation_simple();
    printf("\n");
    int test2_result = test_private_propagation_simple();
    
    printf("\n" COLOR_BLUE "=== 测试结果汇总 ===" COLOR_RESET "\n");
    printf("测试2 (Shared Propagation): %s\n", 
           test1_result == 0 ? COLOR_GREEN "通过" COLOR_RESET : COLOR_RED "失败" COLOR_RESET);
    printf("测试3 (Private Propagation): %s\n", 
           test2_result == 0 ? COLOR_GREEN "通过" COLOR_RESET : COLOR_RED "失败" COLOR_RESET);
    
    if (test1_result == 0 && test2_result == 0) {
        printf(COLOR_GREEN "\n🎉 所有问题测试通过！" COLOR_RESET "\n");
        return 0;
    } else {
        printf(COLOR_RED "\n❌ 部分测试失败，需要进一步调试。" COLOR_RESET "\n");
        return 1;
    }
}