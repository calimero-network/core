from enum import Enum


class BackupKeyAnswerHolderKind(str, Enum):
    ACCOUNT = "account"
    OPERATOR = "operator"
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
