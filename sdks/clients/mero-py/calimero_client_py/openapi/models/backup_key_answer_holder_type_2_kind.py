from enum import Enum


class BackupKeyAnswerHolderType2Kind(str, Enum):
    OPERATOR = "operator"

    def __str__(self) -> str:
        return str(self.value)
