from enum import Enum


class UpdateReasonType3Code(str, Enum):
    MEMBERADDED = "memberAdded"

    def __str__(self) -> str:
        return str(self.value)
