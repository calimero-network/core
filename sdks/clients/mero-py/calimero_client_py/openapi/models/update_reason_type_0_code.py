from enum import Enum


class UpdateReasonType0Code(str, Enum):
    SIGNERCHANGED = "signerChanged"

    def __str__(self) -> str:
        return str(self.value)
