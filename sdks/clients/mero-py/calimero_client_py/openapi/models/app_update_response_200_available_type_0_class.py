from enum import Enum


class AppUpdateResponse200AvailableType0Class(str, Enum):
    GREEN = "green"
    RED = "red"

    def __str__(self) -> str:
        return str(self.value)
