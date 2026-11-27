from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PipelineRunStepFan")


@_attrs_define
class PipelineRunStepFan:
    """
    Attributes:
        total (int):
        items (int):
        batch (int):
        allowed (int):
    """

    total: int
    items: int
    batch: int
    allowed: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        total = self.total

        items = self.items

        batch = self.batch

        allowed = self.allowed

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "total": total,
                "items": items,
                "batch": batch,
                "allowed": allowed,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        total = d.pop("total")

        items = d.pop("items")

        batch = d.pop("batch")

        allowed = d.pop("allowed")

        pipeline_run_step_fan = cls(
            total=total,
            items=items,
            batch=batch,
            allowed=allowed,
        )

        pipeline_run_step_fan.additional_properties = d
        return pipeline_run_step_fan

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
