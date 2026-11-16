"""Generated types from WASM ABI schema"""

from typing import Dict, List, Any, Optional, Union
from dataclasses import dataclass
from enum import Enum


@dataclass
class AbiState:
    counters: Dict[str, int]
    users: List[Any]


@dataclass
class Person:
    id: Any
    name: str
    age: int


@dataclass
class Profile:
    bio: Optional[str]
    avatar: Optional[bytes]
    nicknames: List[str]


@dataclass
class UpdatePayload:
    age: int


class Action(Enum):
    Ping = "Ping"
    SetName = "SetName"
    Update = "Update"


class ConformanceError(Enum):
    BadInput = "BadInput"
    NotFound = "NotFound"
