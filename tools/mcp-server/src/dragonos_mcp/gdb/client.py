"""GDB客户端模块"""

import socket
import re
from typing import List, Dict, Optional


class GDBClient:
    """GDB客户端，连接到localhost:1234"""
    
    def __init__(self):
        self.sock: Optional[socket.socket] = None
        self.connected = False
    
    def connect(self, host: str = "localhost", port: int = 1234) -> bool:
        """连接到GDB服务器"""
        try:
            self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.sock.settimeout(5.0)
            self.sock.connect((host, port))
            self.connected = True
            return True
        except Exception as e:
            print(f"GDB连接失败: {e}")
            return False
    
    def _send_packet(self, data: str) -> bool:
        """发送GDB远程协议数据包"""
        if not self.connected or not self.sock:
            return False
        
        # GDB远程协议格式: $<data>#<checksum>
        checksum = sum(ord(c) for c in data) % 256
        packet = f"${data}#{checksum:02x}".encode('ascii')
        
        try:
            self.sock.sendall(packet)
            # 等待确认
            ack = self.sock.recv(1)
            return ack == b'+'
        except Exception as e:
            print(f"发送数据包失败: {e}")
            return False
    
    def _receive_packet(self, timeout: float = 5.0) -> Optional[str]:
        """接收GDB远程协议数据包"""
        if not self.connected or not self.sock:
            return None
        
        try:
            self.sock.settimeout(timeout)
            
            # 读取直到找到$符号
            while True:
                char = self.sock.recv(1)
                if char == b'$':
                    break
                if not char:
                    return None
            
            # 读取数据直到#
            data = b""
            while True:
                char = self.sock.recv(1)
                if char == b'#':
                    break
                if not char:
                    return None
                data += char
            
            # 读取校验和（2字节）
            try:
                checksum = self.sock.recv(2)
            except socket.timeout:
                checksum = b'00'  # 默认校验和
            
            # 发送确认
            try:
                self.sock.sendall(b'+')
            except:
                pass  # 忽略发送确认失败
            
            return data.decode('utf-8', errors='ignore')
        except socket.timeout:
            return None
        except Exception as e:
            print(f"接收数据包失败: {e}")
            return None
    
    def send_command(self, command: str) -> Optional[str]:
        """发送GDB命令"""
        if not self.connected:
            return None
        
        # 转义特殊字符（GDB远程协议转义）
        escaped = command.replace('#', '#').replace('$', '$').replace('}', '}')
        
        if self._send_packet(escaped):
            return self._receive_packet(timeout=5.0)
        return None
    
    def get_backtrace(self, thread_id: Optional[int] = None, full: bool = False) -> Dict:
        """获取调用栈"""
        if not self.connected:
            return {"error": "未连接到GDB"}
        
        result = {
            "threads": [],
            "backtraces": {}
        }
        
        # 注意：GDB远程协议可能不支持所有命令
        # 这里使用简化的方法，通过pexpect或直接使用gdb命令可能更可靠
        # 暂时返回错误提示，建议使用make gdb手动调试
        
        # 尝试获取线程信息（如果支持）
        try:
            threads_info = self.send_command("qThreadInfo")
            if threads_info and not threads_info.startswith("E"):
                # 解析线程信息
                thread_ids = re.findall(r'([0-9a-f]+)', threads_info)
                result["threads"] = [int(tid, 16) for tid in thread_ids[:10]]  # 限制前10个
        except:
            pass
        
        # 尝试获取调用栈（如果支持）
        try:
            if thread_id is not None:
                # 切换到指定线程
                self.send_command(f"Hg{thread_id:x}")
            
            # 发送bt命令
            bt_cmd = "bt" if not full else "bt"
            bt_output = self.send_command(bt_cmd)
            
            if bt_output and not bt_output.startswith("E"):
                if thread_id is not None:
                    result["backtraces"][thread_id] = bt_output
                else:
                    result["backtraces"][0] = bt_output  # 默认线程0
        except Exception as e:
            result["error"] = f"获取调用栈失败: {str(e)}"
            result["note"] = "建议使用 'make gdb' 手动调试，或使用pexpect实现更完整的GDB集成"
        
        return result
    
    def disconnect(self):
        """断开连接"""
        if self.sock:
            self.sock.close()
            self.sock = None
        self.connected = False

