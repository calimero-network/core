from enum import Enum


class BackupKeyAnswerHolderType1Kind(str, Enum):
    ACCOUNT = "account"

    def __str__(self) -> str:
        return str(self.value)
