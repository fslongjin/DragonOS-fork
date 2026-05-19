#include <gtest/gtest.h>

#include <dirent.h>
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

namespace {

int ensure_dir(const char* path) {
    struct stat st = {};
    if (stat(path, &st) == 0) {
        return S_ISDIR(st.st_mode) ? 0 : -1;
    }
    return mkdir(path, 0755);
}

void remove_tree(const char* root) {
    char path[256] = {};

    snprintf(path, sizeof(path), "%s/m", root);
    umount(path);
    rmdir(path);
    snprintf(path, sizeof(path), "%s/u/x", root);
    rmdir(path);
    snprintf(path, sizeof(path), "%s/l/x", root);
    unlink(path);
    snprintf(path, sizeof(path), "%s/u", root);
    rmdir(path);
    snprintf(path, sizeof(path), "%s/l", root);
    rmdir(path);
    snprintf(path, sizeof(path), "%s/w", root);
    rmdir(path);
    rmdir(root);
}

void alarm_handler(int) {
    _exit(124);
}

}  // namespace

TEST(OverlayFsSemantics, ListAndLookupUpperDirOverLowerFile) {
    char root[128] = {};
    char upper[160] = {};
    char lower[160] = {};
    char work[160] = {};
    char merged[160] = {};
    char upper_x[192] = {};
    char lower_x[192] = {};
    char merged_x[192] = {};
    char options[512] = {};

    snprintf(root, sizeof(root), "/tmp/overlayfs_semantics_%d", getpid());
    snprintf(upper, sizeof(upper), "%s/u", root);
    snprintf(lower, sizeof(lower), "%s/l", root);
    snprintf(work, sizeof(work), "%s/w", root);
    snprintf(merged, sizeof(merged), "%s/m", root);
    snprintf(upper_x, sizeof(upper_x), "%s/x", upper);
    snprintf(lower_x, sizeof(lower_x), "%s/x", lower);
    snprintf(merged_x, sizeof(merged_x), "%s/x", merged);

    ASSERT_EQ(0, ensure_dir("/tmp"));
    ASSERT_EQ(0, ensure_dir(root));
    ASSERT_EQ(0, ensure_dir(upper));
    ASSERT_EQ(0, ensure_dir(lower));
    ASSERT_EQ(0, ensure_dir(work));
    ASSERT_EQ(0, ensure_dir(merged));
    ASSERT_EQ(0, mkdir(upper_x, 0755));

    FILE* lower_file = fopen(lower_x, "w");
    ASSERT_NE(nullptr, lower_file) << strerror(errno);
    fclose(lower_file);

    snprintf(options, sizeof(options), "lowerdir=%s,upperdir=%s,workdir=%s", lower, upper, work);
    if (mount("overlay", merged, "overlay", 0, options) != 0) {
        remove_tree(root);
        GTEST_SKIP() << strerror(errno);
    }

    signal(SIGALRM, alarm_handler);
    alarm(5);

    DIR* dir = opendir(merged);
    if (dir != nullptr) {
        while (readdir(dir) != nullptr) {
        }
        closedir(dir);
    } else if (errno != ENOSYS) {
        FAIL() << strerror(errno);
    }

    struct stat st = {};
    ASSERT_EQ(0, stat(merged_x, &st)) << strerror(errno);
    EXPECT_TRUE(S_ISDIR(st.st_mode));

    alarm(0);
    remove_tree(root);
}

int main(int argc, char** argv) {
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
