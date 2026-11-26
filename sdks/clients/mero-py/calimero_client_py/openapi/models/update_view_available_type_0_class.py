from enum import Enum


class UpdateViewAvailableType0Class(str, Enum):
    GREEN = "green"
    RED = "red"

    def __str__(self) -> str:
        return str(self.value)
