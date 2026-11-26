from enum import Enum


class PipelineCallerType2Kind(str, Enum):
    PUBLIC = "public"

    def __str__(self) -> str:
        return str(self.value)
