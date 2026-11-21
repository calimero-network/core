from enum import Enum


class ConsentPageCreateBodyType19Kind(str, Enum):
    UPDATEPOLICY = "updatePolicy"

    def __str__(self) -> str:
        return str(self.value)
