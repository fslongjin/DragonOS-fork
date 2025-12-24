"""GDB调用栈分析模块"""

import re
from typing import Dict, List


def analyze_backtrace(backtrace: str) -> Dict:
    """分析调用栈，检测死锁和等待问题"""
    result = {
        "type": "unknown",
        "analysis": "",
        "suggestions": []
    }
    
    # 检测死锁模式
    lock_patterns = [
        r"mutex.*lock",
        r"spinlock",
        r"rwlock",
        r"semaphore",
    ]
    
    wait_patterns = [
        r"wait",
        r"sleep",
        r"block",
        r"queue",
        r"futex",
    ]
    
    lock_count = 0
    wait_count = 0
    
    for pattern in lock_patterns:
        if re.search(pattern, backtrace, re.IGNORECASE):
            lock_count += 1
    
    for pattern in wait_patterns:
        if re.search(pattern, backtrace, re.IGNORECASE):
            wait_count += 1
    
    # 分析结果
    if lock_count >= 2:
        result["type"] = "deadlock"
        result["analysis"] = "检测到多个锁操作，可能存在死锁"
        result["suggestions"] = [
            "检查锁的获取顺序是否一致",
            "检查是否有未释放的锁",
            "使用死锁检测工具进一步分析"
        ]
    elif wait_count > 0:
        result["type"] = "wait"
        result["analysis"] = "检测到等待操作，可能正在等待资源或事件"
        result["suggestions"] = [
            "检查等待的条件是否会被满足",
            "检查是否有对应的唤醒操作",
            "检查超时设置是否合理"
        ]
    else:
        result["type"] = "other"
        result["analysis"] = "未检测到明显的死锁或等待模式"
        result["suggestions"] = [
            "查看完整的调用栈信息",
            "检查是否有panic或异常",
            "检查系统资源使用情况"
        ]
    
    return result


