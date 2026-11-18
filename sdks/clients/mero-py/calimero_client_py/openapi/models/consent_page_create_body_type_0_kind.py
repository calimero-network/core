from enum import Enum


class ConsentPageCreateBodyType0Kind(str, Enum):
    RELEASE = "release"

    def __str__(self) -> str:
        return str(self.value)
