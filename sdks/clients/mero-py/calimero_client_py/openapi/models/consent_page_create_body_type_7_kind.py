from enum import Enum


class ConsentPageCreateBodyType7Kind(str, Enum):
    REMOVEMEMBER = "removeMember"

    def __str__(self) -> str:
        return str(self.value)
