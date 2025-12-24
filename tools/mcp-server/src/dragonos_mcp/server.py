#!/usr/bin/env python3
"""DragonOS MCP服务器主程序"""

import asyncio
import os
import sys
from pathlib import Path

# 添加项目根目录到路径
project_root = Path(__file__).parent.parent.parent.parent.parent
sys.path.insert(0, str(project_root / "tools" / "mcp-server" / "src"))

from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Tool, TextContent

from dragonos_mcp.mcp.tools import register_tools
from dragonos_mcp.qemu.process import QEMUManager
from dragonos_mcp.serial.client import SerialClient


# 获取项目根目录
def get_project_root() -> Path:
    """获取DragonOS项目根目录"""
    dragonos_root = os.environ.get("DRAGONOS_ROOT")
    if dragonos_root:
        return Path(dragonos_root)
    # 尝试从当前文件位置推断
    current = Path(__file__).resolve()
    # tools/mcp-server/src/dragonos_mcp/server.py -> 项目根目录
    if "tools" in current.parts:
        idx = current.parts.index("tools")
        return Path(*current.parts[:idx])
    return Path.cwd()


PROJECT_ROOT = get_project_root()


async def main():
    """MCP服务器主函数"""
    # 创建MCP服务器实例
    app = Server("dragonos-mcp")
    
    # 初始化管理器
    qemu_manager = QEMUManager(PROJECT_ROOT)
    
    # 注册工具
    await register_tools(app, qemu_manager, PROJECT_ROOT)
    
    # 启动服务器（使用stdio传输）
    async with stdio_server() as (read_stream, write_stream):
        await app.run(
            read_stream,
            write_stream,
            app.create_initialization_options()
        )


if __name__ == "__main__":
    asyncio.run(main())


