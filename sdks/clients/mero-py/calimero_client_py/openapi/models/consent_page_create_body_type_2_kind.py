from enum import Enum


class ConsentPageCreateBodyType2Kind(str, Enum):
    OUTBOUND = "outbound"

    def __str__(self) -> str:
        return str(self.value)
