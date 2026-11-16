from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.views_body_consistency import ViewsBodyConsistency
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="ViewsBody")


@_attrs_define
class ViewsBody:
    """
    Attributes:
        method (str):
        args (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for whose
            it is).
        consistency (Union[Unset, ViewsBodyConsistency]):
        min_index (Union[Unset, int]):
    """

    method: str
    args: Union[Unset, Any] = UNSET
    consistency: Union[Unset, ViewsBodyConsistency] = UNSET
    min_index: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        method = self.method

        args = self.args

        consistency: Union[Unset, str] = UNSET
        if not isinstance(self.consistency, Unset):
            consistency = self.consistency.value

        min_index = self.min_index

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "method": method,
            }
        )
        if args is not UNSET:
            field_dict["args"] = args
        if consistency is not UNSET:
            field_dict["consistency"] = consistency
        if min_index is not UNSET:
            field_dict["minIndex"] = min_index

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        method = d.pop("method")

        args = d.pop("args", UNSET)

        _consistency = d.pop("consistency", UNSET)
        consistency: Union[Unset, ViewsBodyConsistency]
        if isinstance(_consistency, Unset):
            consistency = UNSET
        else:
            consistency = ViewsBodyConsistency(_consistency)

        min_index = d.pop("minIndex", UNSET)

        views_body = cls(
            method=method,
            args=args,
            consistency=consistency,
            min_index=min_index,
        )

        views_body.additional_properties = d
        return views_body

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
