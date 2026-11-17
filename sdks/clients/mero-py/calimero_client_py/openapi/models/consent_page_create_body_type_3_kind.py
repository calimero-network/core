from enum import Enum


class ConsentPageCreateBodyType3Kind(str, Enum):
    DELEGATE = "delegate"

    def __str__(self) -> str:
        return str(self.value)
