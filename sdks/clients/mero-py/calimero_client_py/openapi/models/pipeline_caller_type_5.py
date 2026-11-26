from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.pipeline_caller_type_5_kind import PipelineCallerType5Kind

T = TypeVar("T", bound="PipelineCallerType5")


@_attrs_define
class PipelineCallerType5:
    """
    Attributes:
        kind (PipelineCallerType5Kind):
        link (str):
        key (str):
    """

    kind: PipelineCallerType5Kind
    link: str
    key: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        link = self.link

        key = self.key

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
                "link": link,
                "key": key,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = PipelineCallerType5Kind(d.pop("kind"))

        link = d.pop("link")

        key = d.pop("key")

        pipeline_caller_type_5 = cls(
            kind=kind,
            link=link,
            key=key,
        )

        pipeline_caller_type_5.additional_properties = d
        return pipeline_caller_type_5

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
