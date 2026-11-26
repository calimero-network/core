from enum import Enum


class PipelineCallerType5Kind(str, Enum):
    VISITOR = "visitor"

    def __str__(self) -> str:
        return str(self.value)
