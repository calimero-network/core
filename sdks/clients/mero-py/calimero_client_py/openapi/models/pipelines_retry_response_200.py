from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="PipelinesRetryResponse200")


@_attrs_define
class PipelinesRetryResponse200:
    """
    Attributes:
        run (str):
        retry_of (str):
        worst_case (Union[Unset, str]):
    """

    run: str
    retry_of: str
    worst_case: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        run = self.run

        retry_of = self.retry_of

        worst_case = self.worst_case

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "run": run,
                "retryOf": retry_of,
            }
        )
        if worst_case is not UNSET:
            field_dict["worstCase"] = worst_case

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        run = d.pop("run")

        retry_of = d.pop("retryOf")

        worst_case = d.pop("worstCase", UNSET)

        pipelines_retry_response_200 = cls(
            run=run,
            retry_of=retry_of,
            worst_case=worst_case,
        )

        pipelines_retry_response_200.additional_properties = d
        return pipelines_retry_response_200

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
