from enum import Enum


class OpsRegisterNodeBodyRole(str, Enum):
    JOB = "job"
    REPLICA = "replica"

    def __str__(self) -> str:
        return str(self.value)
