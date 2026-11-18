from enum import Enum


class RunState(str, Enum):
    CANCELLED = "cancelled"
    DONE = "done"
    FAILED = "failed"
    QUEUED = "queued"
    RUNNING = "running"

    def __str__(self) -> str:
        return str(self.value)
