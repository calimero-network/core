from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.run_queue_reason import RunQueueReason
from ..models.run_queue_state import RunQueueState
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="RunQueue")


@_attrs_define
class RunQueue:
    """
    Attributes:
        state (RunQueueState):
        capacity (int):
        pool (str):
        reason (Union[Unset, RunQueueReason]):
    """

    state: RunQueueState
    capacity: int
    pool: str
    reason: Union[Unset, RunQueueReason] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        state = self.state.value

        capacity = self.capacity

        pool = self.pool

        reason: Union[Unset, str] = UNSET
        if not isinstance(self.reason, Unset):
            reason = self.reason.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "state": state,
                "capacity": capacity,
                "pool": pool,
            }
        )
        if reason is not UNSET:
            field_dict["reason"] = reason

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        state = RunQueueState(d.pop("state"))

        capacity = d.pop("capacity")

        pool = d.pop("pool")

        _reason = d.pop("reason", UNSET)
        reason: Union[Unset, RunQueueReason]
        if isinstance(_reason, Unset):
            reason = UNSET
        else:
            reason = RunQueueReason(_reason)

        run_queue = cls(
            state=state,
            capacity=capacity,
            pool=pool,
            reason=reason,
        )

        run_queue.additional_properties = d
        return run_queue

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
