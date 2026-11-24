from enum import Enum


class ConsentPageCreateBodyType20Kind(str, Enum):
    UPDATEPOLICY = "updatePolicy"

    def __str__(self) -> str:
        return str(self.value)
