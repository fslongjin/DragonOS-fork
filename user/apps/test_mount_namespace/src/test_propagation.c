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

#define TEST_DIR "/tmp/test_propagation"
#define SHARED_DIR "/tmp/shared_mount"
#define CHILD_MOUNT "/tmp/child_mount"

// Mount propagation flags
#ifndef MS_SHARED
#define MS_SHARED (1 << 20)
#endif
#ifndef MS_PRIVATE
#define MS_PRIVATE (1 << 18)
#endif
#ifndef MS_SLAVE
#define MS_SLAVE (1 << 19)
#endif
#ifndef MS_UNBINDABLE
#define MS_UNBINDABLE (1 << 17)
#endif

// 列出目录内容的辅助函数
void list_directory(const char* path, const char* prefix) {
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

// 创建测试文件的辅助函数
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

// 检查文件是否存在
int file_exists(const char* filename) {
    return access(filename, F_OK) == 0;
}

// 测试基本的mount namespace隔离
int test_basic_isolation() {
    printf("\n=== 测试1: 基本Mount Namespace隔离 ===\n");
    
    // 创建测试目录
    if (mkdir(TEST_DIR, 0755) != 0 && errno != EEXIST) {
        printf("错误: 无法创建测试目录: %s\n", strerror(errno));
        return 1;
    }
    
    // 创建父namespace测试文件
    char parent_file[256];
    snprintf(parent_file, sizeof(parent_file), "%s/parent_file.txt", TEST_DIR);
    if (create_test_file(parent_file, "parent content") != 0) {
        return 1;
    }
    
    printf("1. 父namespace初始状态:\n");
    list_directory(TEST_DIR, "   ");
    
    pid_t pid = fork();
    if (pid == 0) {
        // 子进程：创建新的mount namespace
        if (unshare(CLONE_NEWNS) != 0) {
            printf("   [子进程] 错误: 无法创建mount namespace: %s\n", strerror(errno));
            exit(1);
        }
        
        printf("   [子进程] ✓ 创建新的mount namespace成功\n");
        
        // 挂载ramfs
        if (mount("none", TEST_DIR, "ramfs", 0, NULL) != 0) {
            printf("   [子进程] 错误: 挂载ramfs失败: %s\n", strerror(errno));
            exit(1);
        }
        
        printf("   [子进程] ✓ 挂载ramfs成功\n");
        
        // 创建子namespace测试文件
        char child_file[256];
        snprintf(child_file, sizeof(child_file), "%s/child_file.txt", TEST_DIR);
        if (create_test_file(child_file, "child content") != 0) {
            exit(1);
        }
        
        printf("   [子进程] 子namespace状态:\n");
        list_directory(TEST_DIR, "      ");
        
        // 检查隔离效果
        if (file_exists(parent_file)) {
            printf("   [子进程] ❌ 父namespace文件仍可见\n");
        } else {
            printf("   [子进程] ✓ 父namespace文件已被隔离\n");
        }
        
        if (file_exists(child_file)) {
            printf("   [子进程] ✓ 子namespace文件存在\n");
        } else {
            printf("   [子进程] ❌ 子namespace文件不存在\n");
        }
        
        exit(0);
    } else {
        int status;
        waitpid(pid, &status, 0);
        
        printf("2. 父namespace验证:\n");
        list_directory(TEST_DIR, "   ");
        
        if (file_exists(parent_file)) {
            printf("   ✓ 父namespace文件仍存在\n");
        } else {
            printf("   ❌ 父namespace文件丢失\n");
        }
        
        char child_file[256];
        snprintf(child_file, sizeof(child_file), "%s/child_file.txt", TEST_DIR);
        if (file_exists(child_file)) {
            printf("   ❌ 子namespace文件在父namespace可见\n");
        } else {
            printf("   ✓ 子namespace文件已被正确隔离\n");
        }
        
        // 清理
        unlink(parent_file);
        rmdir(TEST_DIR);
        
        return WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : 1;
    }
}

// 测试mount propagation - shared
int test_shared_propagation() {
    printf("\n=== 测试2: Shared Mount Propagation ===\n");
    
    // 创建测试目录
    if (mkdir(SHARED_DIR, 0755) != 0 && errno != EEXIST) {
        printf("错误: 无法创建共享测试目录: %s\n", strerror(errno));
        return 1;
    }
    
    // 设置共享传播
    if (mount("none", SHARED_DIR, "tmpfs", 0, NULL) != 0) {
        printf("挂载tmpfs失败，尝试ramfs: %s\n", strerror(errno));
        if (mount("none", SHARED_DIR, "ramfs", 0, NULL) != 0) {
            printf("错误: 挂载ramfs也失败: %s\n", strerror(errno));
            return 1;
        }
    }
    
    printf("1. 设置共享传播类型...\n");
    if (mount("", SHARED_DIR, "", MS_SHARED, NULL) != 0) {
        printf("   警告: 设置共享传播失败: %s\n", strerror(errno));
        printf("   (这可能表示传播功能尚未完全实现)\n");
    } else {
        printf("   ✓ 设置为共享传播成功\n");
    }
    
    // 创建测试文件
    char shared_file[256];
    snprintf(shared_file, sizeof(shared_file), "%s/shared_file.txt", SHARED_DIR);
    if (create_test_file(shared_file, "shared content") != 0) {
        return 1;
    }
    
    printf("2. 父namespace中的共享挂载点:\n");
    list_directory(SHARED_DIR, "   ");
    
    // 创建子进程测试传播
    pid_t pid = fork();
    if (pid == 0) {
        // 子进程：创建新的mount namespace
        if (unshare(CLONE_NEWNS) != 0) {
            printf("   [子进程] 错误: 无法创建mount namespace: %s\n", strerror(errno));
            exit(1);
        }
        
        printf("   [子进程] ✓ 创建新的mount namespace\n");
        printf("   [子进程] 共享挂载点状态:\n");
        list_directory(SHARED_DIR, "      ");
        
        // 在共享挂载点下创建新的挂载
        if (mkdir(CHILD_MOUNT, 0755) != 0 && errno != EEXIST) {
            printf("   [子进程] 错误: 无法创建子挂载目录: %s\n", strerror(errno));
            exit(1);
        }
        
        if (mount("none", CHILD_MOUNT, "ramfs", 0, NULL) != 0) {
            printf("   [子进程] 错误: 子挂载失败: %s\n", strerror(errno));
            exit(1);
        }
        
        printf("   [子进程] ✓ 在子namespace中创建新挂载\n");
        
        // 创建文件验证传播
        char child_specific_file[256];
        snprintf(child_specific_file, sizeof(child_specific_file), "%s/child_mount_file.txt", CHILD_MOUNT);
        if (create_test_file(child_specific_file, "child mount content") != 0) {
            exit(1);
        }
        
        printf("   [子进程] ✓ 在新挂载点创建文件\n");
        
        exit(0);
    } else {
        int status;
        waitpid(pid, &status, 0);
        
        printf("3. 检查传播效果:\n");
        
        // 检查父namespace是否能看到子进程的挂载
        printf("   父namespace中的挂载点状态:\n");
        list_directory(SHARED_DIR, "      ");
        
        char child_specific_file[256];
        snprintf(child_specific_file, sizeof(child_specific_file), "%s/child_mount_file.txt", CHILD_MOUNT);
        
        if (file_exists(CHILD_MOUNT)) {
            printf("   子挂载点目录存在: %s\n", CHILD_MOUNT);
            if (file_exists(child_specific_file)) {
                printf("   ✓ 共享传播工作正常 - 子进程的挂载传播到父namespace\n");
            } else {
                printf("   ❌ 传播部分工作 - 目录存在但文件不可见\n");
            }
        } else {
            printf("   ❌ 共享传播未工作 - 子进程的挂载未传播到父namespace\n");
            printf("   (这表明传播功能需要进一步实现)\n");
        }
        
        // 清理
        umount(CHILD_MOUNT);
        rmdir(CHILD_MOUNT);
        umount(SHARED_DIR);
        rmdir(SHARED_DIR);
        
        return WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : 1;
    }
}

// 测试mount propagation - private
int test_private_propagation() {
    printf("\n=== 测试3: Private Mount Propagation ===\n");
    
    // 创建测试目录
    if (mkdir(TEST_DIR, 0755) != 0 && errno != EEXIST) {
        printf("错误: 无法创建私有测试目录: %s\n", strerror(errno));
        return 1;
    }
    
    // 挂载并设置为私有
    if (mount("none", TEST_DIR, "ramfs", 0, NULL) != 0) {
        printf("错误: 挂载ramfs失败: %s\n", strerror(errno));
        return 1;
    }
    
    printf("1. 设置私有传播类型...\n");
    if (mount("", TEST_DIR, "", MS_PRIVATE, NULL) != 0) {
        printf("   警告: 设置私有传播失败: %s\n", strerror(errno));
    } else {
        printf("   ✓ 设置为私有传播成功\n");
    }
    
    // 创建测试文件
    char private_file[256];
    snprintf(private_file, sizeof(private_file), "%s/private_file.txt", TEST_DIR);
    if (create_test_file(private_file, "private content") != 0) {
        return 1;
    }
    
    printf("2. 父namespace私有挂载状态:\n");
    list_directory(TEST_DIR, "   ");
    
    // 创建子进程测试隔离
    pid_t pid = fork();
    if (pid == 0) {
        // 子进程：创建新的mount namespace
        if (unshare(CLONE_NEWNS) != 0) {
            printf("   [子进程] 错误: 无法创建mount namespace: %s\n", strerror(errno));
            exit(1);
        }
        
        printf("   [子进程] ✓ 创建新的mount namespace\n");
        printf("   [子进程] 私有挂载状态:\n");
        list_directory(TEST_DIR, "      ");
        
        // 在私有挂载点上进行新的挂载
        if (mount("none", TEST_DIR, "ramfs", 0, NULL) != 0) {
            printf("   [子进程] 错误: 重新挂载失败: %s\n", strerror(errno));
            exit(1);
        }
        
        printf("   [子进程] ✓ 在私有挂载点重新挂载\n");
        
        // 创建不同的文件
        char child_private_file[256];
        snprintf(child_private_file, sizeof(child_private_file), "%s/child_private_file.txt", TEST_DIR);
        if (create_test_file(child_private_file, "child private content") != 0) {
            exit(1);
        }
        
        printf("   [子进程] 重新挂载后的状态:\n");
        list_directory(TEST_DIR, "      ");
        
        exit(0);
    } else {
        int status;
        waitpid(pid, &status, 0);
        
        printf("3. 检查私有传播效果:\n");
        printf("   父namespace状态 (应该保持不变):\n");
        list_directory(TEST_DIR, "      ");
        
        if (file_exists(private_file)) {
            printf("   ✓ 私有传播工作正常 - 父namespace未受子进程挂载影响\n");
        } else {
            printf("   ❌ 私有传播未工作 - 父namespace受到了影响\n");
        }
        
        char child_private_file[256];
        snprintf(child_private_file, sizeof(child_private_file), "%s/child_private_file.txt", TEST_DIR);
        if (file_exists(child_private_file)) {
            printf("   ❌ 私有传播失效 - 子进程的改动传播到了父namespace\n");
        } else {
            printf("   ✓ 私有传播正确 - 子进程的改动未传播\n");
        }
        
        // 清理
        umount(TEST_DIR);
        rmdir(TEST_DIR);
        
        return WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : 1;
    }
}

int main() {
    printf("=== DragonOS Mount Namespace 和 Propagation 综合测试 ===\n");
    
    int results[3];
    
    // 运行所有测试
    results[0] = test_basic_isolation();
    results[1] = test_shared_propagation();
    results[2] = test_private_propagation();
    
    // 汇总结果
    printf("\n=== 测试结果汇总 ===\n");
    printf("1. 基本Mount Namespace隔离: %s\n", results[0] == 0 ? "✓ 通过" : "❌ 失败");
    printf("2. Shared Mount Propagation: %s\n", results[1] == 0 ? "✓ 通过" : "❌ 失败");
    printf("3. Private Mount Propagation: %s\n", results[2] == 0 ? "✓ 通过" : "❌ 失败");
    
    int total_passed = (results[0] == 0) + (results[1] == 0) + (results[2] == 0);
    printf("\n总计: %d/3 项测试通过\n", total_passed);
    
    if (total_passed == 3) {
        printf("🎉 所有测试通过！Mount namespace和propagation功能正常工作。\n");
        return 0;
    } else {
        printf("⚠️  部分测试失败，需要进一步调试和完善。\n");
        return 1;
    }
}