from enum import Enum


class PipelineCallerType0Kind(str, Enum):
    ACCOUNT = "account"

    def __str__(self) -> str:
        return str(self.value)
