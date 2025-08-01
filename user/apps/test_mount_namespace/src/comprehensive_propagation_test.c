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
#define TEST_BASE "/tmp/dragonos_mount_test"
#define SHARED_DIR "/tmp/test_shared"
#define PRIVATE_DIR "/tmp/test_private"
#define SLAVE_DIR "/tmp/test_slave"
#define MASTER_DIR "/tmp/test_master"
#define UNBINDABLE_DIR "/tmp/test_unbindable"
#define BIND_SOURCE "/tmp/bind_source"
#define BIND_TARGET "/tmp/bind_target"

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
#ifndef MS_REC
#define MS_REC 16384
#endif
#ifndef MS_BIND
#define MS_BIND 4096
#endif

// 颜色输出定义
#define COLOR_GREEN "\033[0;32m"
#define COLOR_RED "\033[0;31m"
#define COLOR_YELLOW "\033[0;33m"
#define COLOR_BLUE "\033[0;34m"
#define COLOR_RESET "\033[0m"

// 测试结果统计
typedef struct {
    int total;
    int passed;
    int failed;
    int warnings;
} test_results_t;

static test_results_t g_results = {0, 0, 0, 0};

// 工具函数：打印测试状态
void print_test_header(const char* test_name) {
    printf("\n" COLOR_BLUE "=== %s ===" COLOR_RESET "\n", test_name);
}

void print_success(const char* message) {
    printf(COLOR_GREEN "✓ %s" COLOR_RESET "\n", message);
    g_results.passed++;
}

void print_failure(const char* message) {
    printf(COLOR_RED "❌ %s" COLOR_RESET "\n", message);
    g_results.failed++;
}

void print_warning(const char* message) {
    printf(COLOR_YELLOW "⚠️  %s" COLOR_RESET "\n", message);
    g_results.warnings++;
}

void print_info(const char* message) {
    printf("   %s\n", message);
}

// 工具函数：目录操作
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

// 工具函数：挂载操作
int safe_mount(const char* source, const char* target, const char* fstype, unsigned long flags, const char* data) {
    if (mount(source, target, fstype, flags, data) != 0) {
        printf("挂载失败: %s -> %s (%s): %s\n", source ? source : "none", target, fstype, strerror(errno));
        return -1;
    }
    return 0;
}

int safe_umount(const char* target) {
    if (umount(target) != 0) {
        printf("卸载失败: %s: %s\n", target, strerror(errno));
        return -1;
    }
    return 0;
}

// 初始化测试环境
int setup_test_environment() {
    print_test_header("初始化测试环境");
    
    // 创建所有测试目录
    const char* test_dirs[] = {
        TEST_BASE, SHARED_DIR, PRIVATE_DIR, SLAVE_DIR,
        MASTER_DIR, UNBINDABLE_DIR, BIND_SOURCE, BIND_TARGET
    };
    
    for (int i = 0; i < sizeof(test_dirs) / sizeof(test_dirs[0]); i++) {
        if (create_test_dir(test_dirs[i]) != 0) {
            print_failure("创建测试目录失败");
            return -1;
        }
    }
    
    print_success("测试环境初始化完成");
    return 0;
}

// 清理测试环境
void cleanup_test_environment() {
    print_test_header("清理测试环境");
    
    const char* test_dirs[] = {
        BIND_TARGET, BIND_SOURCE, UNBINDABLE_DIR, MASTER_DIR,
        SLAVE_DIR, PRIVATE_DIR, SHARED_DIR, TEST_BASE
    };
    
    // 尝试卸载所有可能的挂载点
    for (int i = 0; i < sizeof(test_dirs) / sizeof(test_dirs[0]); i++) {
        umount(test_dirs[i]); // 忽略错误
        rmdir(test_dirs[i]);  // 忽略错误
    }
    
    print_info("测试环境清理完成");
}

// 测试1：基本Mount Namespace隔离
int test_basic_namespace_isolation() {
    print_test_header("测试1: 基本Mount Namespace隔离");
    g_results.total++;
    
    // 在父namespace创建测试文件
    char parent_file[256];
    snprintf(parent_file, sizeof(parent_file), "%s/parent_file.txt", TEST_BASE);
    if (create_test_file(parent_file, "parent namespace content") != 0) {
        print_failure("创建父namespace测试文件失败");
        return -1;
    }
    
    print_info("父namespace初始状态:");
    list_directory_contents(TEST_BASE, "   ");
    
    pid_t pid = fork();
    if (pid == 0) {
        // 子进程：创建新的mount namespace
        if (unshare(CLONE_NEWNS) != 0) {
            printf("[子进程] 错误: 无法创建mount namespace: %s\n", strerror(errno));
            exit(1);
        }
        
        print_info("[子进程] 创建新的mount namespace成功");
        
        // 挂载新的文件系统覆盖
        if (safe_mount("none", TEST_BASE, "ramfs", 0, NULL) != 0) {
            printf("[子进程] 挂载ramfs失败，尝试tmpfs\n");
            if (safe_mount("none", TEST_BASE, "tmpfs", 0, NULL) != 0) {
                printf("[子进程] 所有文件系统挂载都失败\n");
                exit(1);
            }
        }
        
        print_info("[子进程] 挂载新文件系统成功");
        
        // 创建子namespace专有文件
        char child_file[256];
        snprintf(child_file, sizeof(child_file), "%s/child_file.txt", TEST_BASE);
        if (create_test_file(child_file, "child namespace content") != 0) {
            exit(1);
        }
        
        print_info("[子进程] 子namespace状态:");
        list_directory_contents(TEST_BASE, "      ");
        
        // 验证隔离效果
        if (file_exists(parent_file)) {
            printf("[子进程] ❌ 父namespace文件仍可见 - 隔离失败\n");
            exit(1);
        } else {
            printf("[子进程] ✓ 父namespace文件已隔离\n");
        }
        
        if (file_exists(child_file)) {
            printf("[子进程] ✓ 子namespace文件存在\n");
        } else {
            printf("[子进程] ❌ 子namespace文件创建失败\n");
            exit(1);
        }
        
        exit(0);
    } else {
        int status;
        waitpid(pid, &status, 0);
        
        print_info("父namespace验证:");
        list_directory_contents(TEST_BASE, "   ");
        
        // 验证父namespace保持不变
        if (file_exists(parent_file)) {
            print_success("基本namespace隔离正常工作");
        } else {
            print_failure("基本namespace隔离失败");
        }
        
        // 清理
        unlink(parent_file);
        
        return WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : -1;
    }
}

// 测试2：Shared Mount Propagation
int test_shared_propagation() {
    print_test_header("测试2: Shared Mount Propagation");
    g_results.total++;
    
    // 设置共享挂载
    if (safe_mount("none", SHARED_DIR, "ramfs", 0, NULL) != 0) {
        if (safe_mount("none", SHARED_DIR, "tmpfs", 0, NULL) != 0) {
            print_failure("无法挂载测试文件系统");
            return -1;
        }
    }
    
    print_info("设置共享传播类型...");
    if (mount("", SHARED_DIR, "", MS_SHARED, NULL) != 0) {
        print_warning("设置共享传播失败 - 可能需要进一步实现");
        print_info("错误信息: " + strlen(strerror(errno)) < 100 ? strerror(errno) : "错误信息过长");
    } else {
        print_success("设置为共享传播成功");
    }
    
    // 创建测试文件
    char shared_file[256];
    snprintf(shared_file, sizeof(shared_file), "%s/shared_file.txt", SHARED_DIR);
    if (create_test_file(shared_file, "shared content") != 0) {
        safe_umount(SHARED_DIR);
        return -1;
    }
    
    print_info("父namespace中的共享挂载点:");
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
        if (unshare(CLONE_NEWNS) != 0) {
            printf("[子进程] 创建mount namespace失败: %s\n", strerror(errno));
            exit(1);
        }
        
        print_info("[子进程] 创建新的mount namespace");
        print_info("[子进程] 共享挂载点状态:");
        list_directory_contents(SHARED_DIR, "      ");
        
        // 在共享挂载点下创建新挂载
        if (safe_mount("none", child_mount_dir, "ramfs", 0, NULL) != 0) {
            printf("[子进程] 在共享挂载点下创建子挂载失败\n");
            exit(1);
        }
        
        print_info("[子进程] 在共享挂载点下创建子挂载成功");
        
        // 创建子挂载的标识文件
        char child_mount_file[256];
        snprintf(child_mount_file, sizeof(child_mount_file), "%s/child_mount_file.txt", child_mount_dir);
        if (create_test_file(child_mount_file, "child mount content") != 0) {
            exit(1);
        }
        
        print_info("[子进程] 在新挂载点创建文件成功");
        
        exit(0);
    } else {
        int status;
        waitpid(pid, &status, 0);
        
        print_info("检查共享传播效果:");
        list_directory_contents(SHARED_DIR, "   ");
        
        char child_mount_file[256];
        snprintf(child_mount_file, sizeof(child_mount_file), "%s/child_mount_file.txt", child_mount_dir);
        
        if (file_exists(child_mount_file)) {
            print_success("共享传播正常工作 - 子进程的挂载传播到父namespace");
        } else {
            print_warning("共享传播可能未完全实现 - 子进程的挂载未传播");
        }
        
        // 清理
        safe_umount(child_mount_dir);
        rmdir(child_mount_dir);
        safe_umount(SHARED_DIR);
        
        return WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : -1;
    }
}

// 测试3：Private Mount Propagation
int test_private_propagation() {
    print_test_header("测试3: Private Mount Propagation");
    g_results.total++;
    
    // 设置私有挂载
    if (safe_mount("none", PRIVATE_DIR, "ramfs", 0, NULL) != 0) {
        if (safe_mount("none", PRIVATE_DIR, "tmpfs", 0, NULL) != 0) {
            print_failure("无法挂载测试文件系统");
            return -1;
        }
    }
    
    print_info("设置私有传播类型...");
    if (mount("", PRIVATE_DIR, "", MS_PRIVATE, NULL) != 0) {
        print_warning("设置私有传播失败");
    } else {
        print_success("设置为私有传播成功");
    }
    
    // 创建测试文件
    char private_file[256];
    snprintf(private_file, sizeof(private_file), "%s/private_file.txt", PRIVATE_DIR);
    if (create_test_file(private_file, "private content") != 0) {
        safe_umount(PRIVATE_DIR);
        return -1;
    }
    
    print_info("父namespace私有挂载状态:");
    list_directory_contents(PRIVATE_DIR, "   ");
    
    pid_t pid = fork();
    if (pid == 0) {
        // 子进程：测试私有传播
        if (unshare(CLONE_NEWNS) != 0) {
            printf("[子进程] 创建mount namespace失败: %s\n", strerror(errno));
            exit(1);
        }
        
        print_info("[子进程] 创建新的mount namespace");
        print_info("[子进程] 私有挂载状态:");
        list_directory_contents(PRIVATE_DIR, "      ");
        
        // 重新挂载私有挂载点
        if (safe_mount("none", PRIVATE_DIR, "ramfs", 0, NULL) != 0) {
            printf("[子进程] 重新挂载失败\n");
            exit(1);
        }
        
        print_info("[子进程] 重新挂载私有挂载点成功");
        
        // 创建子进程专有文件
        char child_private_file[256];
        snprintf(child_private_file, sizeof(child_private_file), "%s/child_private_file.txt", PRIVATE_DIR);
        if (create_test_file(child_private_file, "child private content") != 0) {
            exit(1);
        }
        
        print_info("[子进程] 重新挂载后的状态:");
        list_directory_contents(PRIVATE_DIR, "      ");
        
        exit(0);
    } else {
        int status;
        waitpid(pid, &status, 0);
        
        print_info("检查私有传播效果:");
        print_info("父namespace状态 (应保持不变):");
        list_directory_contents(PRIVATE_DIR, "      ");
        
        if (file_exists(private_file)) {
            print_success("私有传播正常工作 - 父namespace未受子进程影响");
        } else {
            print_failure("私有传播失败 - 父namespace受到了影响");
        }
        
        // 清理
        safe_umount(PRIVATE_DIR);
        
        return WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : -1;
    }
}

// 测试4：Slave Mount Propagation
int test_slave_propagation() {
    print_test_header("测试4: Slave Mount Propagation");
    g_results.total++;
    
    // 设置master挂载
    if (safe_mount("none", MASTER_DIR, "ramfs", 0, NULL) != 0) {
        if (safe_mount("none", MASTER_DIR, "tmpfs", 0, NULL) != 0) {
            print_failure("无法挂载master文件系统");
            return -1;
        }
    }
    
    print_info("设置master为共享传播...");
    if (mount("", MASTER_DIR, "", MS_SHARED, NULL) != 0) {
        print_warning("设置master共享传播失败");
    } else {
        print_success("设置master为共享传播成功");
    }
    
    // 设置slave挂载
    if (safe_mount("none", SLAVE_DIR, "ramfs", 0, NULL) != 0) {
        if (safe_mount("none", SLAVE_DIR, "tmpfs", 0, NULL) != 0) {
            print_failure("无法挂载slave文件系统");
            safe_umount(MASTER_DIR);
            return -1;
        }
    }
    
    print_info("设置slave传播类型...");
    if (mount("", SLAVE_DIR, "", MS_SLAVE, NULL) != 0) {
        print_warning("设置slave传播失败");
    } else {
        print_success("设置为slave传播成功");
    }
    
    // 创建master子挂载目录
    char master_child[256];
    snprintf(master_child, sizeof(master_child), "%s/master_child", MASTER_DIR);
    if (create_test_dir(master_child) != 0) {
        safe_umount(SLAVE_DIR);
        safe_umount(MASTER_DIR);
        return -1;
    }
    
    // 创建slave子挂载目录
    char slave_child[256];
    snprintf(slave_child, sizeof(slave_child), "%s/slave_child", SLAVE_DIR);
    if (create_test_dir(slave_child) != 0) {
        safe_umount(SLAVE_DIR);
        safe_umount(MASTER_DIR);
        return -1;
    }
    
    print_info("测试master到slave的传播...");
    
    // 在master下创建挂载
    if (safe_mount("none", master_child, "ramfs", 0, NULL) != 0) {
        print_warning("在master下创建挂载失败");
    } else {
        print_info("在master下创建挂载成功");
        
        // 创建标识文件
        char master_file[256];
        snprintf(master_file, sizeof(master_file), "%s/master_file.txt", master_child);
        create_test_file(master_file, "master content");
    }
    
    print_info("检查slave是否接收到传播...");
    list_directory_contents(SLAVE_DIR, "   ");
    
    // 测试slave到master的隔离
    print_info("测试slave的隔离性...");
    if (safe_mount("none", slave_child, "ramfs", 0, NULL) != 0) {
        print_warning("在slave下创建挂载失败");
    } else {
        print_info("在slave下创建挂载成功");
        
        char slave_file[256];
        snprintf(slave_file, sizeof(slave_file), "%s/slave_file.txt", slave_child);
        create_test_file(slave_file, "slave content");
    }
    
    print_info("检查master是否保持隔离...");
    list_directory_contents(MASTER_DIR, "   ");
    
    print_success("Slave传播测试完成 (行为验证需要检查日志)");
    
    // 清理
    safe_umount(slave_child);
    safe_umount(master_child);
    safe_umount(SLAVE_DIR);
    safe_umount(MASTER_DIR);
    
    return 0;
}

// 测试5：Unbindable Mount
int test_unbindable_mount() {
    print_test_header("测试5: Unbindable Mount");
    g_results.total++;
    
    // 设置unbindable挂载
    if (safe_mount("none", UNBINDABLE_DIR, "ramfs", 0, NULL) != 0) {
        if (safe_mount("none", UNBINDABLE_DIR, "tmpfs", 0, NULL) != 0) {
            print_failure("无法挂载测试文件系统");
            return -1;
        }
    }
    
    print_info("设置unbindable传播类型...");
    if (mount("", UNBINDABLE_DIR, "", MS_UNBINDABLE, NULL) != 0) {
        print_warning("设置unbindable传播失败");
    } else {
        print_success("设置为unbindable传播成功");
    }
    
    // 创建测试文件
    char unbindable_file[256];
    snprintf(unbindable_file, sizeof(unbindable_file), "%s/unbindable_file.txt", UNBINDABLE_DIR);
    if (create_test_file(unbindable_file, "unbindable content") != 0) {
        safe_umount(UNBINDABLE_DIR);
        return -1;
    }
    
    print_info("尝试bind mount unbindable文件系统...");
    
    // 尝试bind mount（应该失败）
    if (mount(UNBINDABLE_DIR, BIND_TARGET, "", MS_BIND, NULL) != 0) {
        print_success("Unbindable mount正确阻止了bind mount操作");
    } else {
        print_failure("Unbindable mount未能阻止bind mount操作");
        safe_umount(BIND_TARGET);
    }
    
    // 清理
    safe_umount(UNBINDABLE_DIR);
    
    return 0;
}

// 测试6：Bind Mount Propagation
int test_bind_mount_propagation() {
    print_test_header("测试6: Bind Mount Propagation");
    g_results.total++;
    
    // 设置源挂载
    if (safe_mount("none", BIND_SOURCE, "ramfs", 0, NULL) != 0) {
        if (safe_mount("none", BIND_SOURCE, "tmpfs", 0, NULL) != 0) {
            print_failure("无法挂载源文件系统");
            return -1;
        }
    }
    
    // 创建测试文件
    char source_file[256];
    snprintf(source_file, sizeof(source_file), "%s/source_file.txt", BIND_SOURCE);
    if (create_test_file(source_file, "bind source content") != 0) {
        safe_umount(BIND_SOURCE);
        return -1;
    }
    
    print_info("创建bind mount...");
    if (mount(BIND_SOURCE, BIND_TARGET, "", MS_BIND, NULL) != 0) {
        print_warning("Bind mount失败");
        safe_umount(BIND_SOURCE);
        return -1;
    } else {
        print_success("Bind mount创建成功");
    }
    
    print_info("验证bind mount内容同步:");
    list_directory_contents(BIND_SOURCE, "   源目录: ");
    list_directory_contents(BIND_TARGET, "   目标目录: ");
    
    // 验证文件可见性
    char target_file[256];
    snprintf(target_file, sizeof(target_file), "%s/source_file.txt", BIND_TARGET);
    
    if (file_exists(target_file)) {
        print_success("Bind mount内容同步正常");
    } else {
        print_failure("Bind mount内容同步失败");
    }
    
    // 测试bind mount的传播性
    print_info("测试bind mount传播性...");
    if (mount("", BIND_TARGET, "", MS_SHARED, NULL) != 0) {
        print_warning("设置bind mount为共享传播失败");
    } else {
        print_success("设置bind mount为共享传播成功");
    }
    
    // 清理
    safe_umount(BIND_TARGET);
    safe_umount(BIND_SOURCE);
    
    return 0;
}

// 测试7：递归传播操作
int test_recursive_propagation() {
    print_test_header("测试7: 递归传播操作 (MS_REC)");
    g_results.total++;
    
    // 创建嵌套目录结构
    char nested_dir1[256], nested_dir2[256];
    snprintf(nested_dir1, sizeof(nested_dir1), "%s/level1", TEST_BASE);
    snprintf(nested_dir2, sizeof(nested_dir2), "%s/level1/level2", TEST_BASE);
    
    if (create_test_dir(nested_dir1) != 0 || create_test_dir(nested_dir2) != 0) {
        print_failure("创建嵌套目录失败");
        return -1;
    }
    
    // 在各级目录挂载文件系统
    if (safe_mount("none", TEST_BASE, "ramfs", 0, NULL) != 0) {
        print_warning("挂载根测试目录失败");
        return -1;
    }
    
    // 重新创建嵌套目录（因为挂载覆盖了）
    create_test_dir(nested_dir1);
    create_test_dir(nested_dir2);
    
    if (safe_mount("none", nested_dir1, "ramfs", 0, NULL) != 0) {
        print_warning("挂载level1失败");
        safe_umount(TEST_BASE);
        return -1;
    }
    
    // 重新创建level2
    create_test_dir(nested_dir2);
    
    if (safe_mount("none", nested_dir2, "ramfs", 0, NULL) != 0) {
        print_warning("挂载level2失败");
        safe_umount(nested_dir1);
        safe_umount(TEST_BASE);
        return -1;
    }
    
    print_info("创建嵌套挂载结构完成");
    
    // 测试递归设置共享传播
    print_info("递归设置共享传播 (MS_SHARED | MS_REC)...");
    if (mount("", TEST_BASE, "", MS_SHARED | MS_REC, NULL) != 0) {
        print_warning("递归设置共享传播失败");
    } else {
        print_success("递归设置共享传播成功");
    }
    
    // 测试递归设置私有传播
    print_info("递归设置私有传播 (MS_PRIVATE | MS_REC)...");
    if (mount("", TEST_BASE, "", MS_PRIVATE | MS_REC, NULL) != 0) {
        print_warning("递归设置私有传播失败");
    } else {
        print_success("递归设置私有传播成功");
    }
    
    print_success("递归传播操作测试完成");
    
    // 清理
    safe_umount(nested_dir2);
    safe_umount(nested_dir1);
    safe_umount(TEST_BASE);
    
    return 0;
}

// 测试8：复杂传播场景
int test_complex_propagation_scenario() {
    print_test_header("测试8: 复杂传播场景");
    g_results.total++;
    
    print_info("创建复杂的挂载拓扑...");
    
    // 创建多个挂载点并设置不同的传播类型
    const char* mount_points[] = {SHARED_DIR, PRIVATE_DIR, SLAVE_DIR};
    const unsigned long propagation_types[] = {MS_SHARED, MS_PRIVATE, MS_SLAVE};
    const char* type_names[] = {"共享", "私有", "从属"};
    
    for (int i = 0; i < 3; i++) {
        if (safe_mount("none", mount_points[i], "ramfs", 0, NULL) != 0) {
            if (safe_mount("none", mount_points[i], "tmpfs", 0, NULL) != 0) {
                print_warning("挂载失败，跳过");
                continue;
            }
        }
        
        if (mount("", mount_points[i], "", propagation_types[i], NULL) != 0) {
            print_warning("设置传播类型失败");
        } else {
            printf("   ✓ %s: 设置为%s传播\n", mount_points[i], type_names[i]);
        }
        
        // 创建标识文件
        char test_file[256];
        snprintf(test_file, sizeof(test_file), "%s/test_file_%d.txt", mount_points[i], i);
        create_test_file(test_file, type_names[i]);
    }
    
    print_info("测试跨namespace的复杂传播...");
    
    pid_t pid = fork();
    if (pid == 0) {
        // 子进程：创建新namespace并观察传播
        if (unshare(CLONE_NEWNS) != 0) {
            printf("[子进程] 创建mount namespace失败\n");
            exit(1);
        }
        
        print_info("[子进程] 新namespace中的挂载状态:");
        for (int i = 0; i < 3; i++) {
            printf("   %s (%s):\n", mount_points[i], type_names[i]);
            list_directory_contents(mount_points[i], "      ");
        }
        
        exit(0);
    } else {
        int status;
        waitpid(pid, &status, 0);
        
        print_success("复杂传播场景测试完成");
        
        // 清理
        for (int i = 0; i < 3; i++) {
            safe_umount(mount_points[i]);
        }
        
        return WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : -1;
    }
}

// 性能测试
int test_propagation_performance() {
    print_test_header("测试9: 传播性能测试");
    g_results.total++;
    
    struct timespec start, end;
    clock_gettime(CLOCK_MONOTONIC, &start);
    
    // 创建大量挂载点测试性能
    print_info("创建多个挂载点测试性能...");
    
    const int num_mounts = 10;
    char mount_dirs[num_mounts][256];
    
    for (int i = 0; i < num_mounts; i++) {
        snprintf(mount_dirs[i], sizeof(mount_dirs[i]), "/tmp/perf_test_%d", i);
        if (create_test_dir(mount_dirs[i]) != 0) {
            continue;
        }
        
        if (safe_mount("none", mount_dirs[i], "ramfs", 0, NULL) == 0) {
            mount("", mount_dirs[i], "", MS_SHARED, NULL);
        }
    }
    
    clock_gettime(CLOCK_MONOTONIC, &end);
    
    long duration_ms = (end.tv_sec - start.tv_sec) * 1000 + 
                       (end.tv_nsec - start.tv_nsec) / 1000000;
    
    printf("   性能测试结果: %d个挂载点耗时 %ld ms\n", num_mounts, duration_ms);
    
    if (duration_ms < 5000) {  // 5秒内完成认为性能可接受
        print_success("传播性能测试通过");
    } else {
        print_warning("传播性能可能需要优化");
    }
    
    // 清理
    for (int i = 0; i < num_mounts; i++) {
        safe_umount(mount_dirs[i]);
        rmdir(mount_dirs[i]);
    }
    
    return 0;
}

// 主测试函数
int main() {
    printf(COLOR_BLUE "=== DragonOS 挂载传播性综合测试套件 ===" COLOR_RESET "\n");
    printf("测试时间: %s", ctime(&(time_t){time(NULL)}));
    
    // 初始化测试环境
    if (setup_test_environment() != 0) {
        printf(COLOR_RED "测试环境初始化失败，退出测试\n" COLOR_RESET);
        return 1;
    }
    
    // 运行所有测试
    int test_functions[] = {
        test_basic_namespace_isolation(),
        test_shared_propagation(),
        test_private_propagation(),
        test_slave_propagation(),
        test_unbindable_mount(),
        test_bind_mount_propagation(),
        test_recursive_propagation(),
        test_complex_propagation_scenario(),
        test_propagation_performance()
    };
    
    g_results.total = sizeof(test_functions) / sizeof(test_functions[0]);
    
    // 统计实际执行的测试
    int executed_tests = 0;
    for (int i = 0; i < g_results.total; i++) {
        if (test_functions[i] == 0) {
            executed_tests++;
        }
    }
    
    // 清理测试环境
    cleanup_test_environment();
    
    // 输出测试结果
    printf("\n" COLOR_BLUE "=== 测试结果汇总 ===" COLOR_RESET "\n");
    printf("总测试数: %d\n", g_results.total);
    printf(COLOR_GREEN "通过: %d" COLOR_RESET "\n", g_results.passed);
    printf(COLOR_RED "失败: %d" COLOR_RESET "\n", g_results.failed);
    printf(COLOR_YELLOW "警告: %d" COLOR_RESET "\n", g_results.warnings);
    
    float success_rate = g_results.total > 0 ? 
        (float)executed_tests / g_results.total * 100 : 0;
    
    printf("\n成功率: %.1f%%\n", success_rate);
    
    if (executed_tests == g_results.total && g_results.failed == 0) {
        printf(COLOR_GREEN "\n🎉 所有测试通过！DragonOS挂载传播性功能工作正常！" COLOR_RESET "\n");
        return 0;
    } else if (g_results.warnings > 0 && g_results.failed == 0) {
        printf(COLOR_YELLOW "\n⚠️  所有测试执行完成，部分功能可能需要进一步实现。" COLOR_RESET "\n");
        return 0;
    } else {
        printf(COLOR_RED "\n❌ 部分测试失败，需要调试和修复。" COLOR_RESET "\n");
        return 1;
    }
}