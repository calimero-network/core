from enum import Enum


class AccessType1Kind(str, Enum):
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
