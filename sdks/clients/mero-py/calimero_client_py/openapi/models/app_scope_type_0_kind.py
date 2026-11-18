from enum import Enum


class AppScopeType0Kind(str, Enum):
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
