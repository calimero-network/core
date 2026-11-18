from enum import Enum


class TrapKind(str, Enum):
    CANCELLED = "Cancelled"
    FAULT = "Fault"
    HOSTDENIED = "HostDenied"
    OUTOFFUEL = "OutOfFuel"
    OUTOFMEMORY = "OutOfMemory"
    STACKOVERFLOW = "StackOverflow"
    TIMEOUT = "Timeout"
    UNAVAILABLE = "Unavailable"

    def __str__(self) -> str:
        return str(self.value)
