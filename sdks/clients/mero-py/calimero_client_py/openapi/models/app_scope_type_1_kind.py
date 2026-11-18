from enum import Enum


class AppScopeType1Kind(str, Enum):
    ACCOUNT = "account"

    def __str__(self) -> str:
        return str(self.value)
