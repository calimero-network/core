from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.pipeline_caller_type_3_kind import PipelineCallerType3Kind

T = TypeVar("T", bound="PipelineCallerType3")


@_attrs_define
class PipelineCallerType3:
    """
    Attributes:
        kind (PipelineCallerType3Kind):
        app (str):
    """

    kind: PipelineCallerType3Kind
    app: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        app = self.app

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
                "app": app,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = PipelineCallerType3Kind(d.pop("kind"))

        app = d.pop("app")

        pipeline_caller_type_3 = cls(
            kind=kind,
            app=app,
        )

        pipeline_caller_type_3.additional_properties = d
        return pipeline_caller_type_3

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
