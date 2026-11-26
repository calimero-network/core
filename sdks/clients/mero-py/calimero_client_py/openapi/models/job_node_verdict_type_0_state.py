from enum import Enum


class JobNodeVerdictType0State(str, Enum):
    ADMITTED = "admitted"

    def __str__(self) -> str:
        return str(self.value)
