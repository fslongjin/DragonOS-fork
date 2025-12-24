"""QEMU进程管理模块"""

import os
import subprocess
import psutil
import signal
from pathlib import Path
from typing import Optional, Dict, Any
import time

try:
    import grp
    HAS_GRP = True
except ImportError:
    HAS_GRP = False


class QEMUManager:
    """QEMU虚拟机进程管理器"""
    
    def __init__(self, project_root: Path):
        self.project_root = Path(project_root)
        self.arch = os.environ.get("ARCH", "x86_64")
        self.qemu_pid: Optional[int] = None
        self.socket_dir = self.project_root / "bin" / "tmp" / "hypervisor"
        self.serial_socket = self.socket_dir / f"serial-{self.arch}.sock"
        self.monitor_socket = self.socket_dir / f"monitor-{self.arch}.sock"
        
        # 确保socket目录存在
        self.socket_dir.mkdir(parents=True, exist_ok=True)
        
        # 检查权限
        self._check_permissions()
    
    def _check_permissions(self) -> Dict[str, Any]:
        """检查运行QEMU所需的权限"""
        result = {
            "needs_sudo": False,
            "can_run_without_sudo": False,
            "issues": [],
            "suggestions": []
        }
        
        # 检查是否在root用户下运行
        if os.geteuid() == 0:
            result["can_run_without_sudo"] = True
            return result
        
        # 检查是否在kvm组中（通常不需要sudo）
        if HAS_GRP:
            try:
                kvm_gid = grp.getgrnam("kvm").gr_gid
                if kvm_gid in os.getgroups():
                    result["can_run_without_sudo"] = True
                    return result
            except (KeyError, OSError):
                pass  # kvm组不存在或无法访问
        
        # 检查/dev/kvm权限
        if os.path.exists("/dev/kvm"):
            try:
                kvm_stat = os.stat("/dev/kvm")
                if kvm_stat.st_mode & 0o006:  # 其他用户有读写权限
                    result["can_run_without_sudo"] = True
                    return result
            except:
                pass
        
        # 检查sudo免密配置
        try:
            # 测试sudo是否可以无密码执行
            test_result = subprocess.run(
                ["sudo", "-n", "true"],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=2
            )
            if test_result.returncode == 0:
                result["needs_sudo"] = True
                result["sudo_nopasswd"] = True
                return result
            else:
                result["needs_sudo"] = True
                result["sudo_nopasswd"] = False
                result["issues"].append("需要sudo权限，但未配置免密")
                result["suggestions"].append(
                    "配置sudo免密：运行 'sudo visudo' 并添加：\n"
                    f"  {os.getenv('USER', 'your_username')} ALL=(ALL) NOPASSWD: /usr/bin/qemu-system-*, /usr/bin/rm"
                )
        except (subprocess.TimeoutExpired, FileNotFoundError):
            result["needs_sudo"] = True
            result["sudo_nopasswd"] = False
            result["issues"].append("无法检测sudo配置")
        
        return result
    
    def get_socket_paths(self) -> Dict[str, str]:
        """获取socket路径"""
        return {
            "serial_socket": str(self.serial_socket),
            "monitor_socket": str(self.monitor_socket),
        }
    
    def find_qemu_process(self) -> Optional[int]:
        """查找运行中的QEMU进程"""
        for proc in psutil.process_iter(['pid', 'name', 'cmdline']):
            try:
                cmdline = proc.info.get('cmdline', [])
                if cmdline and any('qemu-system' in str(arg) for arg in cmdline):
                    # 检查是否是当前架构的QEMU
                    if f"qemu-system-{self.arch}" in ' '.join(cmdline):
                        return proc.info['pid']
            except (psutil.NoSuchProcess, psutil.AccessDenied):
                continue
        return None
    
    def is_running(self) -> bool:
        """检查QEMU是否正在运行"""
        if self.qemu_pid:
            try:
                proc = psutil.Process(self.qemu_pid)
                if proc.is_running():
                    return True
            except psutil.NoSuchProcess:
                pass
        
        # 尝试查找进程
        pid = self.find_qemu_process()
        if pid:
            self.qemu_pid = pid
            return True
        return False
    
    def start(self, timeout: int = 300) -> Dict[str, Any]:
        """启动QEMU虚拟机"""
        if self.is_running():
            return {
                "pid": self.qemu_pid,
                "status": "already_running",
                "serial_socket": str(self.serial_socket),
                "monitor_socket": str(self.monitor_socket),
            }
        
        # 检查权限
        perm_check = self._check_permissions()
        if perm_check["needs_sudo"] and not perm_check.get("sudo_nopasswd", False):
            return {
                "pid": None,
                "status": "failed",
                "error": "需要sudo权限但未配置免密",
                "permission_check": perm_check,
                "suggestion": (
                    "请配置sudo免密或手动运行一次 'make run-nographic' 输入密码。\n"
                    "配置sudo免密：运行 'sudo visudo' 并添加：\n"
                    f"  {os.getenv('USER', 'your_username')} ALL=(ALL) NOPASSWD: /usr/bin/qemu-system-*, /usr/bin/rm"
                )
            }
        
        # 清理旧的socket文件
        if self.serial_socket.exists():
            self.serial_socket.unlink()
        if self.monitor_socket.exists():
            self.monitor_socket.unlink()
        
        # 切换到项目根目录执行 make kernel && make qemu-nographic
        # 注意：MCP服务器只编译内核并启动虚拟机，不编译用户程序或写入rootfs镜像（权限问题）
        try:
            env = os.environ.copy()
            env["ARCH"] = self.arch
            
            # 先编译内核，然后启动QEMU
            # 注意：如果make qemu-nographic需要sudo，这里可能会失败
            # 建议用户预先配置sudo免密
            process = subprocess.Popen(
                ["bash", "-c", "make kernel && make qemu-nographic"],
                cwd=str(self.project_root),
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                preexec_fn=os.setsid  # 创建新的进程组
            )
            
            # 等待socket文件创建（最多等待10秒）
            max_wait = 10
            waited = 0
            while waited < max_wait:
                if self.serial_socket.exists() and self.monitor_socket.exists():
                    break
                time.sleep(0.5)
                waited += 0.5
            
            # 查找QEMU进程
            time.sleep(2)  # 给QEMU一些启动时间
            pid = self.find_qemu_process()
            if pid:
                self.qemu_pid = pid
                return {
                    "pid": pid,
                    "status": "started",
                    "serial_socket": str(self.serial_socket),
                    "monitor_socket": str(self.monitor_socket),
                }
            else:
                # 检查stderr是否有权限错误
                try:
                    _, stderr = process.communicate(timeout=1)
                    stderr_text = stderr.decode('utf-8', errors='ignore')
                    if "sudo" in stderr_text.lower() or "permission denied" in stderr_text.lower():
                        return {
                            "pid": None,
                            "status": "failed",
                            "error": "权限不足，需要sudo",
                            "stderr": stderr_text,
                            "suggestion": (
                                "请配置sudo免密或手动运行一次 'make run-nographic' 输入密码。\n"
                                "配置sudo免密：运行 'sudo visudo' 并添加：\n"
                                f"  {os.getenv('USER', 'your_username')} ALL=(ALL) NOPASSWD: /usr/bin/qemu-system-*, /usr/bin/rm"
                            )
                        }
                except:
                    pass
                
                return {
                    "pid": None,
                    "status": "failed",
                    "error": "QEMU进程未找到，可能启动失败",
                    "suggestion": "请检查 'make run-nographic' 是否可以在终端中正常运行"
                }
        except Exception as e:
            return {
                "pid": None,
                "status": "failed",
                "error": str(e),
            }
    
    def stop(self, force: bool = False) -> Dict[str, Any]:
        """停止QEMU虚拟机"""
        if not self.is_running():
            return {
                "success": True,
                "message": "QEMU未运行",
            }
        
        try:
            proc = psutil.Process(self.qemu_pid)
            
            if force:
                # 强制终止
                proc.kill()
                # 等待进程结束
                try:
                    proc.wait(timeout=5)
                except psutil.TimeoutExpired:
                    pass
            else:
                # 优雅终止
                proc.terminate()
                # 等待进程结束（最多5秒）
                try:
                    proc.wait(timeout=5)
                except psutil.TimeoutExpired:
                    # 超时后强制终止
                    proc.kill()
            
            # 清理socket文件
            if self.serial_socket.exists():
                self.serial_socket.unlink()
            if self.monitor_socket.exists():
                self.monitor_socket.unlink()
            
            self.qemu_pid = None
            return {
                "success": True,
                "message": "QEMU已停止",
            }
        except psutil.NoSuchProcess:
            self.qemu_pid = None
            return {
                "success": True,
                "message": "QEMU进程不存在",
            }
        except Exception as e:
            return {
                "success": False,
                "message": f"停止失败: {str(e)}",
            }
    
    def get_status(self) -> Dict[str, Any]:
        """获取QEMU状态"""
        running = self.is_running()
        status = {
            "running": running,
            "serial_socket": str(self.serial_socket),
            "monitor_socket": str(self.monitor_socket),
        }
        
        if running:
            status["pid"] = self.qemu_pid
        
        # 添加权限检查信息
        perm_check = self._check_permissions()
        status["permissions"] = perm_check
        
        # 读取最后输出（从日志文件）
        log_file = self.project_root / "serial_opt.txt"
        if log_file.exists():
            try:
                with open(log_file, 'r', encoding='utf-8', errors='ignore') as f:
                    lines = f.readlines()
                    if lines:
                        status["last_output"] = ''.join(lines[-20:])  # 最后20行
            except Exception:
                pass
        
        return status

