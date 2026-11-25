from enum import Enum


class JobNodeVerdictType1State(str, Enum):
    REFUSED = "refused"
    UNREACHABLE = "unreachable"

    def __str__(self) -> str:
        return str(self.value)
