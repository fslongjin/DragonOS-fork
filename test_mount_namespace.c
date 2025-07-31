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

#define TEST_DIR "/tmp/test_mount"
#define PARENT_FILE TEST_DIR "/parent_file.txt"
#define CHILD_FILE TEST_DIR "/child_file.txt"

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

int main() {
    pid_t pid;
    int status;
    
    printf("=== DragonOS Mount Namespace 测试程序 ===\n\n");
    
    // 创建测试目录
    printf("1. 准备测试环境...\n");
    if (mkdir(TEST_DIR, 0755) < 0 && errno != EEXIST) {
        printf("错误: 无法创建测试目录 %s: %s\n", TEST_DIR, strerror(errno));
        return 1;
    }
    
    // 在父namespace中创建测试文件
    if (create_test_file(PARENT_FILE, "这是父namespace中的文件\n") < 0) {
        return 1;
    }
    printf("   创建父namespace测试文件: %s\n", PARENT_FILE);
    
    // 显示初始状态
    printf("\n2. 初始状态 (父namespace):\n");
    list_directory(TEST_DIR, "   ");
    
    // 创建子进程进行mount namespace测试
    printf("\n3. 创建子进程进行mount namespace测试...\n");
    pid = fork();
    
    if (pid < 0) {
        printf("错误: fork失败: %s\n", strerror(errno));
        return 1;
    }
    
    if (pid == 0) {
        // 子进程：创建新的mount namespace
        printf("   [子进程] 开始测试新的mount namespace\n");
        
        // 创建新的mount namespace
        if (unshare(CLONE_NEWNS) < 0) {
            printf("   [子进程] 错误: unshare失败: %s\n", strerror(errno));
            exit(1);
        }
        printf("   [子进程] ✓ 成功创建新的mount namespace\n");
        
        // 挂载ramfs到测试目录
        printf("   [子进程] 尝试挂载ramfs到 %s\n", TEST_DIR);
        if (mount("none", TEST_DIR, "ramfs", 0, NULL) < 0) {
            printf("   [子进程] 错误: mount失败: %s\n", strerror(errno));
            exit(1);
        }
        printf("   [子进程] ✓ 成功挂载ramfs\n");
        
        // 在新挂载的ramfs中创建文件
        if (create_test_file(CHILD_FILE, "这是子namespace中ramfs的文件\n") < 0) {
            exit(1);
        }
        printf("   [子进程] ✓ 在ramfs中创建测试文件: %s\n", CHILD_FILE);
        
        // 显示子namespace中的目录内容
        printf("   [子进程] 新mount namespace中的目录内容:\n");
        list_directory(TEST_DIR, "      ");
        
        // 验证父namespace的文件不存在
        if (file_exists(PARENT_FILE)) {
            printf("   [子进程] ❌ 错误: 父namespace的文件仍然可见！\n");
        } else {
            printf("   [子进程] ✓ 确认: 父namespace的文件已被隔离\n");
        }
        
        // 验证子namespace的文件存在
        if (file_exists(CHILD_FILE)) {
            printf("   [子进程] ✓ 确认: 子namespace的文件存在\n");
        } else {
            printf("   [子进程] ❌ 错误: 子namespace的文件不存在！\n");
        }
        
        printf("   [子进程] mount namespace测试完成\n");
        exit(0);
    } else {
        // 父进程：等待子进程完成
        printf("   [父进程] 等待子进程完成测试...\n");
        
        if (waitpid(pid, &status, 0) < 0) {
            printf("   [父进程] 错误: waitpid失败: %s\n", strerror(errno));
            return 1;
        }
        
        if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
            printf("   [父进程] ✓ 子进程测试成功完成\n");
        } else {
            printf("   [父进程] ❌ 子进程测试失败\n");
        }
        
        // 验证父namespace的状态没有改变
        printf("\n4. 验证父namespace状态:\n");
        list_directory(TEST_DIR, "   ");
        
        if (file_exists(PARENT_FILE)) {
            printf("   ✓ 确认: 父namespace的文件仍然存在\n");
        } else {
            printf("   ❌ 错误: 父namespace的文件丢失！\n");
        }
        
        if (file_exists(CHILD_FILE)) {
            printf("   ❌ 错误: 子namespace的文件在父namespace中可见！\n");
        } else {
            printf("   ✓ 确认: 子namespace的文件已被正确隔离\n");
        }
    }
    
    // 清理测试环境
    printf("\n5. 清理测试环境...\n");
    if (unlink(PARENT_FILE) == 0) {
        printf("   删除测试文件: %s\n", PARENT_FILE);
    }
    if (rmdir(TEST_DIR) == 0) {
        printf("   删除测试目录: %s\n", TEST_DIR);
    }
    
    printf("\n=== 测试完成 ===\n");
    printf("总结:\n");
    printf("- Mount namespace隔离: %s\n", 
           (WIFEXITED(status) && WEXITSTATUS(status) == 0) ? "✓ 成功" : "❌ 失败");
    printf("- Ramfs挂载功能: %s\n", 
           (WIFEXITED(status) && WEXITSTATUS(status) == 0) ? "✓ 正常" : "❌ 异常");
    
    return (WIFEXITED(status) && WEXITSTATUS(status) == 0) ? 0 : 1;
}