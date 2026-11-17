from enum import Enum


class ConsentPageCreateBodyType3Kind(str, Enum):
    OUTBOUND = "outbound"

    def __str__(self) -> str:
        return str(self.value)
