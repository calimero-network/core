from enum import Enum


class UpdateReasonType5Code(str, Enum):
    DELEGATEDMETHODSADDED = "delegatedMethodsAdded"

    def __str__(self) -> str:
        return str(self.value)
