from enum import Enum


class PipelineRunStepStatus(str, Enum):
    CANCELLED = "cancelled"
    FAILED = "failed"
    PENDING = "pending"
    RUNNING = "running"
    SKIPPED = "skipped"
    SUCCEEDED = "succeeded"
    UPSTREAMFAILED = "upstreamFailed"

    def __str__(self) -> str:
        return str(self.value)
