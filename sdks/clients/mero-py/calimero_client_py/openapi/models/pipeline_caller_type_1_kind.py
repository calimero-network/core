from enum import Enum


class PipelineCallerType1Kind(str, Enum):
    APIKEY = "apiKey"

    def __str__(self) -> str:
        return str(self.value)
