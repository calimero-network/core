from enum import Enum


class UpdateReasonType2Code(str, Enum):
    STATEVERSIONCHANGED = "stateVersionChanged"

    def __str__(self) -> str:
        return str(self.value)
