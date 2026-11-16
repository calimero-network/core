from enum import Enum


class ConsentPageResultResponse200Status(str, Enum):
    APPROVED = "approved"
    DECLINED = "declined"
    PENDING = "pending"

    def __str__(self) -> str:
        return str(self.value)
