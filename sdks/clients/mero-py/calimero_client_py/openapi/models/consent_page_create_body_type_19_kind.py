from enum import Enum


class ConsentPageCreateBodyType19Kind(str, Enum):
    INSTALL = "install"

    def __str__(self) -> str:
        return str(self.value)
