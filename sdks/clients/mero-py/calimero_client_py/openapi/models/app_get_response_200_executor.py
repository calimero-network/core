from enum import Enum


class AppGetResponse200Executor(str, Enum):
    COMPONENT = "component"
    WASMER = "wasmer"

    def __str__(self) -> str:
        return str(self.value)
