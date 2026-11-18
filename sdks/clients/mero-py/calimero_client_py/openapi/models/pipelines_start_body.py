from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.pipelines_start_body_inputs import PipelinesStartBodyInputs
    from ..models.pipelines_start_body_params import PipelinesStartBodyParams


T = TypeVar("T", bound="PipelinesStartBody")


@_attrs_define
class PipelinesStartBody:
    """
    Attributes:
        inputs (Union[Unset, PipelinesStartBodyInputs]):
        params (Union[Unset, PipelinesStartBodyParams]):
        max_cost (Union[Unset, str]):
    """

    inputs: Union[Unset, "PipelinesStartBodyInputs"] = UNSET
    params: Union[Unset, "PipelinesStartBodyParams"] = UNSET
    max_cost: Union[Unset, str] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.pipelines_start_body_inputs import PipelinesStartBodyInputs
        from ..models.pipelines_start_body_params import PipelinesStartBodyParams

        inputs: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.inputs, Unset):
            inputs = self.inputs.to_dict()

        params: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.params, Unset):
            params = self.params.to_dict()

        max_cost = self.max_cost

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if inputs is not UNSET:
            field_dict["inputs"] = inputs
        if params is not UNSET:
            field_dict["params"] = params
        if max_cost is not UNSET:
            field_dict["maxCost"] = max_cost

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pipelines_start_body_inputs import PipelinesStartBodyInputs
        from ..models.pipelines_start_body_params import PipelinesStartBodyParams

        d = dict(src_dict)
        _inputs = d.pop("inputs", UNSET)
        inputs: Union[Unset, PipelinesStartBodyInputs]
        if isinstance(_inputs, Unset):
            inputs = UNSET
        else:
            inputs = PipelinesStartBodyInputs.from_dict(_inputs)

        _params = d.pop("params", UNSET)
        params: Union[Unset, PipelinesStartBodyParams]
        if isinstance(_params, Unset):
            params = UNSET
        else:
            params = PipelinesStartBodyParams.from_dict(_params)

        max_cost = d.pop("maxCost", UNSET)

        pipelines_start_body = cls(
            inputs=inputs,
            params=params,
            max_cost=max_cost,
        )

        return pipelines_start_body
