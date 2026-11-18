from enum import Enum


class ConsentPageCreateBodyType1Kind(str, Enum):
    UPGRADE = "upgrade"

    def __str__(self) -> str:
        return str(self.value)
