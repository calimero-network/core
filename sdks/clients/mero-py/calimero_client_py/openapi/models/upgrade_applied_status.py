from enum import Enum


class UpgradeAppliedStatus(str, Enum):
    APPERROR = "appError"
    OK = "ok"
    TRAP = "trap"

    def __str__(self) -> str:
        return str(self.value)
