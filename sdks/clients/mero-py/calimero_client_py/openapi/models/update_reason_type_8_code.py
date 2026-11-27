from enum import Enum


class UpdateReasonType8Code(str, Enum):
    UNCLASSIFIED = "unclassified"

    def __str__(self) -> str:
        return str(self.value)
