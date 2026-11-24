from enum import Enum


class ConsentPageCreateBodyType18Kind(str, Enum):
    SETEGRESS = "setEgress"

    def __str__(self) -> str:
        return str(self.value)
