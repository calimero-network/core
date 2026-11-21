from enum import Enum


class ConsentPageCreateBodyType20Kind(str, Enum):
    UPDATES = "updates"

    def __str__(self) -> str:
        return str(self.value)
