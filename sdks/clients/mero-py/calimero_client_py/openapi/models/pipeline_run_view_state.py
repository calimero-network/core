from enum import Enum


class PipelineRunViewState(str, Enum):
    CANCELLED = "cancelled"
    FAILED = "failed"
    RUNNING = "running"
    SUCCEEDED = "succeeded"
    SUCCEEDEDWITHFAILURES = "succeededWithFailures"

    def __str__(self) -> str:
        return str(self.value)
