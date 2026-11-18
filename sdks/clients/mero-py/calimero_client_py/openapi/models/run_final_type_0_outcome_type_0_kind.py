from enum import Enum


class RunFinalType0OutcomeType0Kind(str, Enum):
    DONE = "done"

    def __str__(self) -> str:
        return str(self.value)
