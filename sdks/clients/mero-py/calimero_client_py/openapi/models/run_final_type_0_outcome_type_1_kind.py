from enum import Enum


class RunFinalType0OutcomeType1Kind(str, Enum):
    FAILED = "failed"

    def __str__(self) -> str:
        return str(self.value)
