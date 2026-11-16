from enum import Enum


class RunFinalType0OutcomeType2Kind(str, Enum):
    CANCELLED = "cancelled"

    def __str__(self) -> str:
        return str(self.value)
