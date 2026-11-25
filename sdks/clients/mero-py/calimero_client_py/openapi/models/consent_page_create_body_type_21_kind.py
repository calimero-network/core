from enum import Enum


class ConsentPageCreateBodyType21Kind(str, Enum):
    UPDATES = "updates"

    def __str__(self) -> str:
        return str(self.value)
