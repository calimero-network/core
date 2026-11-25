from enum import Enum


class UpdateReasonType6Code(str, Enum):
    SECRETBINDINGCHANGED = "secretBindingChanged"

    def __str__(self) -> str:
        return str(self.value)
