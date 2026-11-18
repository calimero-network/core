from enum import Enum


class AppExecutor(str, Enum):
    COMPONENT = "component"
    WASMER = "wasmer"

    def __str__(self) -> str:
        return str(self.value)
