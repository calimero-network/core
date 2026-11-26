from enum import Enum


class PipelineCallerType3Kind(str, Enum):
    APP = "app"

    def __str__(self) -> str:
        return str(self.value)
