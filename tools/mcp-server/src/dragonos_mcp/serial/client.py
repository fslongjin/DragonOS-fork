"""串口socket客户端（只读模式）"""

import socket
import select
import time
import re
from pathlib import Path
from typing import Optional, List


class SerialClient:
    """串口socket客户端（只读模式）"""
    
    def __init__(self, socket_path: str):
        self.socket_path = Path(socket_path)
        self.sock: Optional[socket.socket] = None
        self.connected = False
        self.buffer = ""
    
    def connect(self) -> bool:
        """连接到串口socket"""
        if not self.socket_path.exists():
            return False
        
        try:
            self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self.sock.connect(str(self.socket_path))
            self.connected = True
            return True
        except Exception as e:
            print(f"连接失败: {e}")
            return False
    
    def read_output(self, timeout: float = 1.0) -> Optional[str]:
        """读取输出（非阻塞）"""
        if not self.connected or not self.sock:
            return None
        
        ready, _, _ = select.select([self.sock], [], [], timeout)
        if ready:
            try:
                data = self.sock.recv(4096)
                if data:
                    text = data.decode('utf-8', errors='ignore')
                    self.buffer += text
                    return text
            except Exception as e:
                print(f"读取失败: {e}")
                self.connected = False
                return None
        return None
    
    def read_lines(self, num_lines: int = 100, timeout: float = 5.0) -> List[str]:
        """读取指定行数的输出"""
        lines = []
        end_time = time.time() + timeout
        
        # 先处理缓冲区中的内容
        if self.buffer:
            buffer_lines = self.buffer.split('\n')
            lines.extend(buffer_lines[:-1])  # 除了最后一行（可能不完整）
            self.buffer = buffer_lines[-1]
        
        while len(lines) < num_lines and time.time() < end_time:
            output = self.read_output(timeout=0.5)
            if output:
                new_lines = output.split('\n')
                if len(new_lines) > 1:
                    # 合并最后一行到缓冲区
                    lines.append(self.buffer + new_lines[0])
                    self.buffer = new_lines[-1]
                    lines.extend(new_lines[1:-1])
                else:
                    self.buffer += new_lines[0]
            else:
                time.sleep(0.1)
        
        # 如果缓冲区还有内容，添加最后一行
        if self.buffer and len(lines) < num_lines:
            lines.append(self.buffer)
            self.buffer = ""
        
        return lines[:num_lines]
    
    def wait_for_pattern(self, pattern: str, timeout: float = 30.0) -> bool:
        """等待匹配指定模式"""
        import time
        regex = re.compile(pattern)
        end_time = time.time() + timeout
        
        while time.time() < end_time:
            output = self.read_output(timeout=1.0)
            if output:
                if regex.search(self.buffer):
                    return True
            else:
                time.sleep(0.1)
        
        return False
    
    def disconnect(self):
        """断开连接"""
        if self.sock:
            self.sock.close()
            self.sock = None
        self.connected = False
        self.buffer = ""



