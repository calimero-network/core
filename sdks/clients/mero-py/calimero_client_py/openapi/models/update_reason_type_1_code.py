from enum import Enum


class UpdateReasonType1Code(str, Enum):
    EGRESSADDED = "egressAdded"

    def __str__(self) -> str:
        return str(self.value)
