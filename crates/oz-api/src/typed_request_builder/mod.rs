use std::marker::PhantomData;

use reqwest::{RequestBuilder, Response};
use serde::de::DeserializeOwned;

pub struct TypedRequestBuilder<T> {
    builder: RequestBuilder,
    _type:   PhantomData<T>,
}

pub struct TypedResponse<T> {
    response: reqwest::Response,
    _type:    PhantomData<T>,
}

impl<T> TypedRequestBuilder<T> {
    pub fn new(builder: RequestBuilder) -> Self {
        Self {
            builder,
            _type: PhantomData,
        }
    }

    pub async fn send(self) -> Result<TypedResponse<T>, reqwest::Error> {
        let response = self.builder.send().await?;

        Ok(TypedResponse {
            response,
            _type: PhantomData,
        })
    }

    pub fn map(self, f: impl FnOnce(RequestBuilder) -> RequestBuilder) -> Self {
        Self {
            builder: f(self.builder),
            _type:   PhantomData,
        }
    }
}

impl<T> TypedResponse<T> {
    pub fn into_untyped(self) -> Response {
        self.response
    }
}

impl<T> TypedResponse<T>
where
    T: DeserializeOwned,
{
    pub async fn json(self) -> Result<T, reqwest::Error> {
        self.response.json().await
    }
}

impl<T> AsRef<RequestBuilder> for TypedRequestBuilder<T> {
    fn as_ref(&self) -> &RequestBuilder {
        &self.builder
    }
}

impl<T> AsMut<RequestBuilder> for TypedRequestBuilder<T> {
    fn as_mut(&mut self) -> &mut RequestBuilder {
        &mut self.builder
    }
}

impl<T> AsRef<reqwest::Response> for TypedResponse<T> {
    fn as_ref(&self) -> &reqwest::Response {
        &self.response
    }
}

impl<T> AsMut<reqwest::Response> for TypedResponse<T> {
    fn as_mut(&mut self) -> &mut reqwest::Response {
        &mut self.response
    }
}

impl<T> From<RequestBuilder> for TypedRequestBuilder<T> {
    fn from(builder: RequestBuilder) -> Self {
        Self::new(builder)
    }
}
