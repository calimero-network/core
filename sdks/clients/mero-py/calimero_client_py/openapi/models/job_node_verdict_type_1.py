from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.job_node_verdict_type_1_state import JobNodeVerdictType1State

T = TypeVar("T", bound="JobNodeVerdictType1")


@_attrs_define
class JobNodeVerdictType1:
    """
    Attributes:
        state (JobNodeVerdictType1State):
        node (int):
        reason (str):
    """

    state: JobNodeVerdictType1State
    node: int
    reason: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        state = self.state.value

        node = self.node

        reason = self.reason

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "state": state,
                "node": node,
                "reason": reason,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        state = JobNodeVerdictType1State(d.pop("state"))

        node = d.pop("node")

        reason = d.pop("reason")

        job_node_verdict_type_1 = cls(
            state=state,
            node=node,
            reason=reason,
        )

        job_node_verdict_type_1.additional_properties = d
        return job_node_verdict_type_1

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
