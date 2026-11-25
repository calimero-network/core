from enum import Enum


class BackupKeyAnswerHolderType0Kind(str, Enum):
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
