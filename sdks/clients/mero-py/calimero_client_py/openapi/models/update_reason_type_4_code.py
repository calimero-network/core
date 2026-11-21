from enum import Enum


class UpdateReasonType4Code(str, Enum):
    MEMBERREMOVED = "memberRemoved"

    def __str__(self) -> str:
        return str(self.value)
