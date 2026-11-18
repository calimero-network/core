from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.run_final_type_0_outcome_type_1_kind import RunFinalType0OutcomeType1Kind
from ..models.run_final_type_0_outcome_type_1_reason import (
    RunFinalType0OutcomeType1Reason,
)

T = TypeVar("T", bound="RunFinalType0OutcomeType1")


@_attrs_define
class RunFinalType0OutcomeType1:
    """
    Attributes:
        kind (RunFinalType0OutcomeType1Kind):
        reason (RunFinalType0OutcomeType1Reason):
    """

    kind: RunFinalType0OutcomeType1Kind
    reason: RunFinalType0OutcomeType1Reason
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        reason = self.reason.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
                "reason": reason,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = RunFinalType0OutcomeType1Kind(d.pop("kind"))

        reason = RunFinalType0OutcomeType1Reason(d.pop("reason"))

        run_final_type_0_outcome_type_1 = cls(
            kind=kind,
            reason=reason,
        )

        run_final_type_0_outcome_type_1.additional_properties = d
        return run_final_type_0_outcome_type_1

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
