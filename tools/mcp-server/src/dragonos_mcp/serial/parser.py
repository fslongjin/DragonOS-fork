"""串口输出解析模块"""

import re
from typing import Dict, List, Optional


def parse_boot_complete(output: str) -> bool:
    """检查系统是否启动完成"""
    patterns = [
        r"Running system init script",
        r"开始运行gvisor系统调用测试",
        r"/bin/busybox.*init",
        r"console.*enabled",
    ]
    for pattern in patterns:
        if re.search(pattern, output, re.IGNORECASE):
            return True
    return False


def parse_test_output(output: str) -> Dict[str, any]:
    """解析测试输出"""
    result = {
        "success": False,
        "passed": 0,
        "failed": 0,
        "total": 0,
        "failures": [],
    }
    
    # 解析gtest格式的输出
    passed_match = re.search(r"\[  PASSED  \]\s+(\d+)", output)
    failed_match = re.search(r"\[  FAILED  \]\s+(\d+)", output)
    
    if passed_match:
        result["passed"] = int(passed_match.group(1))
    if failed_match:
        result["failed"] = int(failed_match.group(1))
    
    result["total"] = result["passed"] + result["failed"]
    result["success"] = result["failed"] == 0
    
    # 提取失败的测试用例
    failure_pattern = r"\[  FAILED  \]\s+([^\s]+)"
    failures = re.findall(failure_pattern, output)
    result["failures"] = failures
    
    # 解析成功率
    success_rate_match = re.search(r"成功率[:\s]+([\d.]+)%", output)
    if success_rate_match:
        result["success_rate"] = float(success_rate_match.group(1))
    
    return result


def extract_error_messages(output: str) -> List[str]:
    """提取错误消息"""
    errors = []
    
    # 匹配常见的错误模式
    error_patterns = [
        r"ERROR[:\s]+(.+)",
        r"error[:\s]+(.+)",
        r"FAILED[:\s]+(.+)",
        r"panic[:\s]+(.+)",
        r"assertion failed[:\s]+(.+)",
    ]
    
    for pattern in error_patterns:
        matches = re.finditer(pattern, output, re.IGNORECASE | re.MULTILINE)
        for match in matches:
            error_msg = match.group(1).strip()
            if error_msg and error_msg not in errors:
                errors.append(error_msg)
    
    return errors


