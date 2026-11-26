from enum import Enum


class UpdateReasonType7Code(str, Enum):
    PINSLINEAGE = "pinsLineage"

    def __str__(self) -> str:
        return str(self.value)
