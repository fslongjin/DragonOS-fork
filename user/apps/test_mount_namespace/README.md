# DragonOS Mount Namespace 和 Propagation 测试程序

这个目录包含了用于测试DragonOS挂载命名空间和挂载传播性功能的测试程序。

## 测试程序说明

### 1. test_mount_namespace
基本的mount namespace隔离测试程序，验证：
- Mount namespace的基本创建和隔离功能
- 子进程挂载不影响父进程的能力

### 2. test_propagation
简单的传播功能测试程序，基于原有的test_propagation.c，测试：
- 基本的mount namespace隔离
- Shared mount propagation
- Private mount propagation

### 3. comprehensive_propagation_test (推荐)
**全面的挂载传播性测试套件**，包含9个详细测试：

1. **基本Mount Namespace隔离** - 验证namespace基本功能
2. **Shared Mount Propagation** - 测试共享传播（双向）
3. **Private Mount Propagation** - 测试私有传播（完全隔离）
4. **Slave Mount Propagation** - 测试从属传播（单向接收）
5. **Unbindable Mount** - 测试不可绑定挂载
6. **Bind Mount Propagation** - 测试bind mount的传播性
7. **递归传播操作 (MS_REC)** - 测试递归传播标志
8. **复杂传播场景** - 测试多种传播类型的组合
9. **传播性能测试** - 测试大量挂载的性能表现

## 编译说明

### 编译所有测试程序
```bash
make all
```

### 编译特定测试程序
```bash
make test_basic              # 仅编译基本测试
make test_simple             # 仅编译简单传播测试
make test_comprehensive      # 仅编译全面测试套件
make comprehensive           # 同上，别名
```

### 编译调试版本
```bash
make debug
```

### 查看帮助信息
```bash
make help
```

### 清理编译产物
```bash
make clean
```

## 运行测试

### 运行基本测试
```bash
./test_mount_namespace
```

### 运行简单传播测试
```bash
./test_propagation
```

### 运行全面测试套件（推荐）
```bash
./comprehensive_propagation_test
```

## 测试预期结果

### 功能完全实现时
- 所有测试应该显示绿色的"✓"标记
- 最终显示"🎉 所有测试通过！DragonOS挂载传播性功能工作正常！"

### 功能部分实现时
- 部分测试显示黄色的"⚠️"警告
- 最终显示"⚠️ 所有测试执行完成，部分功能可能需要进一步实现。"

### 功能有问题时
- 测试显示红色的"❌"错误
- 最终显示"❌ 部分测试失败，需要调试和修复。"

## 测试详情

### 测试的传播类型
- **MS_SHARED**: 共享传播，双向同步挂载事件
- **MS_PRIVATE**: 私有传播，完全隔离，默认行为
- **MS_SLAVE**: 从属传播，单向接收master的传播
- **MS_UNBINDABLE**: 不可绑定，禁止bind mount操作

### 测试的标志
- **MS_REC**: 递归标志，递归应用传播设置
- **MS_BIND**: Bind mount标志

### 验证的Linux一致性
这些测试程序验证DragonOS的挂载传播行为与Linux内核完全一致，包括：
- 传播语义的正确性
- 跨namespace的传播行为
- 错误处理和边界条件
- 性能特征

## 故障排除

### 如果测试失败
1. 检查是否有足够的权限（需要root权限）
2. 确认DragonOS的挂载传播功能已正确实现
3. 查看详细的错误信息和日志
4. 对比测试行为与Linux内核行为

### 如果出现警告
- 警告通常表示某些高级功能尚未完全实现
- 这是正常的开发过程，可以逐步完善

### 调试建议
1. 运行调试版本：`make debug && ./comprehensive_propagation_test`
2. 查看内核日志中的相关信息
3. 逐个运行测试，isolate问题

## 开发说明

### 添加新测试
1. 在`comprehensive_propagation_test.c`中添加新的测试函数
2. 在`main()`函数中调用新测试
3. 更新测试计数和结果统计

### 修改测试参数
- 调整测试目录路径（如果需要）
- 修改性能测试的挂载数量
- 调整超时和重试逻辑

这些测试程序是验证DragonOS挂载传播性功能实现正确性的重要工具，请在开发过程中经常运行以确保功能的稳定性和Linux兼容性。