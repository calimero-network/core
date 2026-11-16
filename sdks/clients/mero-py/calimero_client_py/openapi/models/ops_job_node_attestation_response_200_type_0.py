from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.ops_job_node_attestation_response_200_type_0_state import (
    OpsJobNodeAttestationResponse200Type0State,
)

T = TypeVar("T", bound="OpsJobNodeAttestationResponse200Type0")


@_attrs_define
class OpsJobNodeAttestationResponse200Type0:
    """
    Attributes:
        state (OpsJobNodeAttestationResponse200Type0State):
        node (int):
        release (str):
    """

    state: OpsJobNodeAttestationResponse200Type0State
    node: int
    release: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        state = self.state.value

        node = self.node

        release = self.release

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "state": state,
                "node": node,
                "release": release,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        state = OpsJobNodeAttestationResponse200Type0State(d.pop("state"))

        node = d.pop("node")

        release = d.pop("release")

        ops_job_node_attestation_response_200_type_0 = cls(
            state=state,
            node=node,
            release=release,
        )

        ops_job_node_attestation_response_200_type_0.additional_properties = d
        return ops_job_node_attestation_response_200_type_0

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
