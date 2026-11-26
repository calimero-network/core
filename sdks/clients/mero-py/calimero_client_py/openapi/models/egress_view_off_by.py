from enum import Enum


class EgressViewOffBy(str, Enum):
    INSTANCE = "instance"
    ORGANIZATION = "organization"

    def __str__(self) -> str:
        return str(self.value)
