from enum import Enum


class ConsentPageCreateBodyType9Kind(str, Enum):
    REMOVEMEMBER = "removeMember"

    def __str__(self) -> str:
        return str(self.value)
