"""MCP工具注册模块"""

import time
from mcp.server import Server
from mcp.types import Tool, TextContent
from pathlib import Path
from typing import Any

from dragonos_mcp.qemu.process import QEMUManager
from dragonos_mcp.serial.client import SerialClient
from dragonos_mcp.serial.parser import parse_boot_complete, parse_test_output
from dragonos_mcp.qemu.monitor import QEMUMonitor
from dragonos_mcp.gdb.client import GDBClient
from dragonos_mcp.gdb.analyzer import analyze_backtrace


async def register_tools(app: Server, qemu_manager: QEMUManager, project_root: Path):
    """注册所有MCP工具"""
    
    @app.list_tools()
    async def list_tools() -> list[Tool]:
        """列出所有可用工具"""
        return [
            Tool(
                name="dragonos_vm_start",
                description=(
                    "启动DragonOS虚拟机。"
                    "注意：由于权限限制，此工具只会编译内核(make kernel)并启动虚拟机(make qemu-nographic)，"
                    "不会编译用户程序或写入rootfs镜像。如果需要更新用户程序或rootfs，请手动运行 'make all && make write_diskimage'。"
                ),
                inputSchema={
                    "type": "object",
                    "properties": {
                        "arch": {
                            "type": "string",
                            "description": "架构，默认x86_64",
                            "default": "x86_64"
                        },
                        "timeout": {
                            "type": "number",
                            "description": "启动超时（秒），默认300",
                            "default": 300
                        }
                    }
                }
            ),
            Tool(
                name="dragonos_vm_stop",
                description="停止DragonOS虚拟机",
                inputSchema={
                    "type": "object",
                    "properties": {
                        "force": {
                            "type": "boolean",
                            "description": "是否强制终止，默认false",
                            "default": False
                        }
                    }
                }
            ),
            Tool(
                name="dragonos_vm_status",
                description="查询虚拟机状态",
                inputSchema={"type": "object", "properties": {}}
            ),
            Tool(
                name="dragonos_vm_get_serial_path",
                description="获取串口socket路径",
                inputSchema={"type": "object", "properties": {}}
            ),
            Tool(
                name="dragonos_serial_read",
                description="读取串口输出",
                inputSchema={
                    "type": "object",
                    "properties": {
                        "lines": {
                            "type": "number",
                            "description": "读取行数，默认100",
                            "default": 100
                        },
                        "timeout": {
                            "type": "number",
                            "description": "超时（秒），默认5",
                            "default": 5
                        },
                        "pattern": {
                            "type": "string",
                            "description": "匹配模式（正则表达式）"
                        }
                    }
                }
            ),
            Tool(
                name="dragonos_wait_boot",
                description="等待系统启动完成",
                inputSchema={
                    "type": "object",
                    "properties": {
                        "timeout": {
                            "type": "number",
                            "description": "超时（秒），默认300",
                            "default": 300
                        }
                    }
                }
            ),
            Tool(
                name="dragonos_test_run",
                description="在虚拟机内执行测试",
                inputSchema={
                    "type": "object",
                    "properties": {
                        "test_path": {
                            "type": "string",
                            "description": "测试程序路径，如/opt/tests/gvisor/socket_test"
                        },
                        "test_dir": {
                            "type": "string",
                            "description": "工作目录，默认/opt/tests/gvisor",
                            "default": "/opt/tests/gvisor"
                        },
                        "timeout": {
                            "type": "number",
                            "description": "超时（秒），默认600",
                            "default": 600
                        }
                    },
                    "required": ["test_path"]
                }
            ),
            Tool(
                name="dragonos_gdb_backtrace",
                description="获取调用栈",
                inputSchema={
                    "type": "object",
                    "properties": {
                        "thread_id": {
                            "type": "number",
                            "description": "线程ID，默认所有线程"
                        },
                        "full": {
                            "type": "boolean",
                            "description": "是否包含局部变量，默认false",
                            "default": False
                        }
                    }
                }
            ),
            Tool(
                name="dragonos_gdb_analyze",
                description="分析卡死原因",
                inputSchema={"type": "object", "properties": {}}
            ),
        ]
    
    @app.call_tool()
    async def call_tool(name: str, arguments: dict[str, Any]) -> list[TextContent]:
        """处理工具调用"""
        
        if name == "dragonos_vm_start":
            arch = arguments.get("arch", "x86_64")
            timeout = arguments.get("timeout", 300)
            qemu_manager.arch = arch
            result = qemu_manager.start(timeout=timeout)
            
            # 添加说明信息
            if isinstance(result, dict):
                result["note"] = (
                    "注意：MCP服务器只编译内核并启动虚拟机，不编译用户程序或写入rootfs镜像（权限问题）。"
                    "如果需要更新用户程序或rootfs，请手动运行 'make all && make write_diskimage'。"
                )
            
            return [TextContent(type="text", text=str(result))]
        
        elif name == "dragonos_vm_stop":
            force = arguments.get("force", False)
            result = qemu_manager.stop(force=force)
            return [TextContent(type="text", text=str(result))]
        
        elif name == "dragonos_vm_status":
            result = qemu_manager.get_status()
            return [TextContent(type="text", text=str(result))]
        
        elif name == "dragonos_vm_get_serial_path":
            paths = qemu_manager.get_socket_paths()
            connect_cmd = f"socat - UNIX-CONNECT:{paths['serial_socket']}"
            result = {
                "socket_path": paths["serial_socket"],
                "connect_command": connect_cmd
            }
            return [TextContent(type="text", text=str(result))]
        
        elif name == "dragonos_serial_read":
            lines = arguments.get("lines", 100)
            timeout = arguments.get("timeout", 5.0)
            pattern = arguments.get("pattern")
            
            client = SerialClient(qemu_manager.serial_socket)
            if not client.connect():
                return [TextContent(type="text", text="无法连接到串口socket")]
            
            try:
                if pattern:
                    matched = client.wait_for_pattern(pattern, timeout=timeout)
                    output = client.buffer
                    return [TextContent(type="text", text=f"匹配: {matched}\n输出:\n{output}")]
                else:
                    output_lines = client.read_lines(num_lines=lines, timeout=timeout)
                    output = "\n".join(output_lines)
                    return [TextContent(type="text", text=output)]
            finally:
                client.disconnect()
        
        elif name == "dragonos_wait_boot":
            timeout = arguments.get("timeout", 300)
            
            client = SerialClient(qemu_manager.serial_socket)
            if not client.connect():
                return [TextContent(type="text", text="无法连接到串口socket")]
            
            try:
                start_time = time.time()
                booted = False
                
                while time.time() - start_time < timeout:
                    output = client.read_output(timeout=1.0)
                    if output and parse_boot_complete(client.buffer):
                        booted = True
                        break
                
                boot_time = time.time() - start_time
                result = {
                    "booted": booted,
                    "boot_time": boot_time
                }
                return [TextContent(type="text", text=str(result))]
            finally:
                client.disconnect()
        
        elif name == "dragonos_test_run":
            test_path = arguments["test_path"]
            test_dir = arguments.get("test_dir", "/opt/tests/gvisor")
            timeout = arguments.get("timeout", 600)
            
            # 使用monitor发送命令
            monitor = QEMUMonitor(qemu_manager.monitor_socket)
            if not monitor.connect():
                return [TextContent(type="text", text="无法连接到monitor socket")]
            
            try:
                # 切换到测试目录
                monitor.send_text(f"cd {test_dir}\n")
                time.sleep(0.5)
                
                # 执行测试
                monitor.send_text(f"./{test_path}\n")
                
                # 读取输出
                client = SerialClient(qemu_manager.serial_socket)
                if client.connect():
                    end_time = time.time() + timeout
                    output_buffer = ""
                    
                    while time.time() < end_time:
                        output = client.read_output(timeout=1.0)
                        if output:
                            output_buffer += output
                            # 检查测试是否完成
                            if "PASSED" in output_buffer or "FAILED" in output_buffer:
                                # 再等待一点时间收集完整输出
                                time.sleep(2)
                                break
                    
                    client.disconnect()
                    
                    # 解析测试结果
                    result = parse_test_output(output_buffer)
                    result["output"] = output_buffer
                    return [TextContent(type="text", text=str(result))]
                else:
                    return [TextContent(type="text", text="无法读取测试输出")]
            finally:
                monitor.disconnect()
        
        elif name == "dragonos_gdb_backtrace":
            thread_id = arguments.get("thread_id")
            full = arguments.get("full", False)
            
            gdb_client = GDBClient()
            if not gdb_client.connect():
                return [TextContent(type="text", text="无法连接到GDB服务器(localhost:1234)")]
            
            try:
                result = gdb_client.get_backtrace(thread_id=thread_id, full=full)
                return [TextContent(type="text", text=str(result))]
            finally:
                gdb_client.disconnect()
        
        elif name == "dragonos_gdb_analyze":
            gdb_client = GDBClient()
            if not gdb_client.connect():
                return [TextContent(type="text", text="无法连接到GDB服务器(localhost:1234)")]
            
            try:
                # 获取所有线程的调用栈
                bt_result = gdb_client.get_backtrace(full=True)
                
                # 分析每个线程的调用栈
                analysis_results = {}
                for tid, backtrace in bt_result.get("backtraces", {}).items():
                    analysis = analyze_backtrace(backtrace)
                    analysis_results[tid] = analysis
                
                # 汇总分析结果
                result = {
                    "threads": bt_result.get("threads", []),
                    "analyses": analysis_results
                }
                
                return [TextContent(type="text", text=str(result))]
            finally:
                gdb_client.disconnect()
        
        else:
            return [TextContent(type="text", text=f"未知工具: {name}")]

