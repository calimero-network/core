from enum import Enum


class PipelineCallerType4Kind(str, Enum):
    SYSTEM = "system"

    def __str__(self) -> str:
        return str(self.value)
