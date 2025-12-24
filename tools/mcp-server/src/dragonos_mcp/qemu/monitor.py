"""QEMU Monitor交互模块"""

import socket
import json
from pathlib import Path
from typing import Optional, Dict, Any
import time


class QEMUMonitor:
    """QEMU Monitor客户端"""
    
    def __init__(self, socket_path: str):
        self.socket_path = Path(socket_path)
        self.sock: Optional[socket.socket] = None
        self.connected = False
    
    def connect(self) -> bool:
        """连接到QEMU monitor socket"""
        if not self.socket_path.exists():
            return False
        
        try:
            self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self.sock.connect(str(self.socket_path))
            self.connected = True
            return True
        except Exception as e:
            print(f"Monitor连接失败: {e}")
            return False
    
    def send_command(self, command: str) -> Optional[str]:
        """发送monitor命令"""
        if not self.connected or not self.sock:
            return None
        
        try:
            # QEMU monitor命令以换行符结束
            self.sock.sendall((command + "\n").encode('utf-8'))
            
            # 读取响应（简单实现，可能需要更复杂的解析）
            time.sleep(0.1)  # 等待响应
            response = b""
            self.sock.settimeout(1.0)
            try:
                while True:
                    data = self.sock.recv(4096)
                    if not data:
                        break
                    response += data
                    if len(data) < 4096:
                        break
            except socket.timeout:
                pass
            
            return response.decode('utf-8', errors='ignore')
        except Exception as e:
            print(f"发送命令失败: {e}")
            return None
    
    def send_key(self, key: str) -> bool:
        """发送键盘按键（使用sendkey命令）"""
        # QEMU sendkey命令格式: sendkey <key>
        # 特殊键需要特殊格式，如: ctrl-a, ctrl-c, ret (回车), spc (空格)
        command = f"sendkey {key}"
        result = self.send_command(command)
        return result is not None
    
    def send_text(self, text: str) -> bool:
        """发送文本（逐字符发送）"""
        for char in text:
            if char == '\n':
                if not self.send_key("ret"):
                    return False
            elif char == ' ':
                if not self.send_key("spc"):
                    return False
            else:
                # 普通字符直接发送
                if not self.send_key(char.lower()):
                    return False
            time.sleep(0.05)  # 字符间延迟
        return True
    
    def disconnect(self):
        """断开连接"""
        if self.sock:
            self.sock.close()
            self.sock = None
        self.connected = False


