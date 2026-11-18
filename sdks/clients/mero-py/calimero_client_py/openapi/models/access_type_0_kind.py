from enum import Enum


class AccessType0Kind(str, Enum):
    OWNER = "owner"

    def __str__(self) -> str:
        return str(self.value)
